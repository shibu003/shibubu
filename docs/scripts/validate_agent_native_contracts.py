#!/usr/bin/env python3
"""Static draft schema+references verification (not native Rust execution).

examples/app_v3_*.json : v0.12 の設計サンプル 3 本（緩い検査のまま・触らない）
examples/v3/*.json     : 生成 app の regression corpus。README（examples/README.md）の規則で厳しく検査する
                         （readiness の嘘・承認 gate の抜け・捏造データ・KPI card を機械で落とす）
"""
from pathlib import Path
import json, sys
from jsonschema import Draft202012Validator
root=Path(__file__).resolve().parents[1]
app=json.loads((root/'schemas/agent_native_app_v3.schema.json').read_text())
proposal=json.loads((root/'schemas/agent_proposal_v1.schema.json').read_text())
for s in (app,proposal): Draft202012Validator.check_schema(s)
for f in sorted((root/'examples').glob('app_v3_*.json')):
 d=json.loads(f.read_text());Draft202012Validator(app).validate(d)
 action_ids=[a['id'] for a in d['actions']]; source_ids=[s['id'] for s in d['sources']]
 assert len(action_ids)==len(set(action_ids)),f'actions duplicate in {f}'
 assert len(source_ids)==len(set(source_ids)),f'sources duplicate in {f}'
 node_keys=[]
 def walk(n):
  node_keys.append(n['key'])
  if 'action_ref' in n: assert n['action_ref'] in action_ids,f'missing action in {f}'
  if 'binding' in n: assert n['binding']['source_ref'] in source_ids,f'missing source in {f}'
  for child in n.get('children',[]): walk(child)
 walk(d['composition']['root']); assert len(node_keys)==len(set(node_keys)),f'duplicate UI keys in {f}'
 assert not any(a['effect'].startswith('external') and a['readiness']=='ready' for a in d['actions'] if d['state']['execution_mode']=='local_demo')
 print('PASS app',f.name,len(node_keys),'nodes')
p=json.loads((root/'examples/agent_proposal_v1.json').read_text());Draft202012Validator(proposal).validate(p)
assert p['app_id']=='example.research'
assert p['target_ref']=='research-root'
print('PASS proposal',p['id'])

# ---- regression corpus: examples/v3/*.json（規則の本文は examples/README.md）----
COLLECTION={'list','table','calendar','timeline','chat','detail'}   # binding するなら states.empty が要る
BANNED={'kpi','metric','stat','chart'}                              # DESIGN §3: 新規 app の KPI card は 0
INLINE_DATA={'items','rows','data','records','messages','events','threads','contacts','replies'}  # 行データは binding でしか来ない
NOT_READY_CAP={'unknown','discoverable','requires_connection','connected_unverified','degraded','revoked'}
SURFACE={'native','embedded','handoff','mirror','composite'}        # DESIGN §4.4

def corpus_errors(d, raw):
 e=[]
 def must(c,m):
  if not c: e.append(m)
 for label,ids in (('actions',[a['id'] for a in d['actions']]),('sources',[s['id'] for s in d['sources']]),
                   ('capabilities',[c['id'] for c in d.get('capabilities',[])]),('workflows',[w['id'] for w in d.get('workflows',[])])):
  must(len(ids)==len(set(ids)),f'duplicate {label} ids')
 acts={a['id']:a for a in d['actions']}; srcs={s['id']:s for s in d['sources']}; caps={c['id']:c for c in d.get('capabilities',[])}
 must('/Users/' not in raw,'absolute home path in fixture')
 must(isinstance(d.get('metadata',{}).get('corpus',{}).get('todo'),list) and d['metadata']['corpus']['todo'],'metadata.corpus.todo missing')
 must(d.get('metadata',{}).get('corpus',{}).get('prompt')==d['goal']['literal_goal'],'metadata.corpus.prompt != goal.literal_goal')
 for a in d['actions']:
  cap=caps.get(a.get('capability_ref'))
  if 'capability_ref' in a: must(cap is not None,f"{a['id']}: unknown capability_ref")
  must(a['readiness']!='preview_only',f"{a['id']}: preview_only は corpus では使わない（ready か requires_connection か unsupported）")
  if a['effect'].startswith('external'): must(cap is not None,f"{a['id']}: external effect needs capability_ref")
  if a['readiness']=='ready': must(cap is None or cap['readiness']=='ready',f"{a['id']}: ready but capability is {cap and cap['readiness']}")
  if a['readiness']=='requires_connection': must(cap is not None and cap['readiness'] in NOT_READY_CAP,f"{a['id']}: requires_connection needs a not-ready capability")
  if a['readiness']=='unsupported': must(cap is not None and cap['readiness']=='unsupported',f"{a['id']}: unsupported needs an unsupported capability")
  if a['effect'] in ('external_send','destructive'): must(a.get('requires_approval') is True,f"{a['id']}: {a['effect']} needs requires_approval=true")
 for s in d['sources']:
  if s['readiness']=='local': must(s['kind'].startswith('local'),f"{s['id']}: readiness local but kind {s['kind']}")
  if s['readiness'] not in ('local','ready'): must(s.get('last_observed_at') is None,f"{s['id']}: not ready but has last_observed_at（捏造した鮮度）")
 for w in d.get('workflows',[]):
  for r in w['action_refs']: must(r in acts,f"workflow {w['id']}: unknown action {r}")
  if any(acts[r]['effect'] in ('external_send','destructive') for r in w['action_refs'] if r in acts):
   must(w.get('approval') not in (None,'','none'),f"workflow {w['id']}: send/destructive step needs approval")
 ready=[a for a in d['actions'] if a['readiness']=='ready']; notready=[a for a in d['actions'] if a['readiness']!='ready']
 ext_ready=[s for s in d['sources'] if s['readiness']=='ready']; src_notready=[s for s in d['sources'] if s['readiness'] not in ('local','ready')]
 mode=d['state']['execution_mode']; st=d['state']['readiness']
 must(mode in ('local_ready','integrated'),f'execution_mode {mode} は corpus では使わない')
 if mode=='integrated': must(ext_ready,'integrated needs a ready external source')
 if mode=='local_ready': must(ready and not ext_ready,'local_ready needs ready local actions and no ready external source')
 must(st in ('ready','partial','blocked'),f'state.readiness {st} は corpus では使わない')
 if st=='ready': must(not notready and not src_notready,'state ready but something is not ready')
 if st=='partial': must(ready and (notready or src_notready),'state partial needs ready and not-ready parts')
 if st=='blocked': must(not ready,'state blocked but a ready action exists')
 keys=set(); count=0
 def walk(n,depth,path):
  nonlocal count
  count+=1; must(depth<=24 and count<=500,'tree resource budget exceeded')
  must(n['key'] not in keys,f"duplicate node key {n['key']}"); keys.add(n['key'])
  c=n['component']; st_=n.get('states',{}); props=n.get('props',{})
  must(c not in BANNED,f"{n['key']}: component {c} は置かない（DESIGN §3 KPI 0）")
  if c=='card': must(props.get('selectable') is True,f"{n['key']}: card は選択できる物だけ")
  if c in ('button','form','command_input','recorder'): must('action_ref' in n,f"{n['key']}: {c} needs action_ref")
  if c=='source_surface': must(props.get('display') in SURFACE,f"{n['key']}: source_surface needs props.display in {sorted(SURFACE)}")
  must(not (set(props)&INLINE_DATA),f"{n['key']}: props に行データ {sorted(set(props)&INLINE_DATA)} を inline で書かない")
  if 'action_ref' in n:
   a=acts.get(n['action_ref']); must(a is not None,f"{n['key']}: unknown action_ref {n['action_ref']}")
   if a and a['readiness']!='ready': must(isinstance(st_.get('disabled'),str) and st_['disabled'],f"{n['key']}: 未接続の action は states.disabled に理由が要る")
  if 'binding' in n:
   s=srcs.get(n['binding']['source_ref']); must(s is not None,f"{n['key']}: unknown source_ref {n['binding']['source_ref']}")
   must(n['binding']['path'],f"{n['key']}: empty binding path")
   if c in COLLECTION: must(isinstance(st_.get('empty'),str) and st_['empty'],f"{n['key']}: bound {c} needs states.empty")
   if s and s['readiness'] not in ('local','ready'): must(isinstance(st_.get('requires_connection'),str) and st_['requires_connection'],f"{n['key']}: bound to unconnected source needs states.requires_connection")
  for ch in n.get('children',[]): walk(ch,depth+1,path+[n['key']])
 walk(d['composition']['root'],1,[])
 return e,count,len(ready),len(d['actions'])

corpus=sorted((root/'examples/v3').glob('*.json'))
readme=(root/'examples/README.md').read_text()
failed=0
for f in corpus:
 d=json.loads(f.read_text()); errs=[]
 try: Draft202012Validator(app).validate(d)
 except Exception as ex: errs.append(f'schema: {str(ex).splitlines()[0]}')
 if not errs: errs,n,nready,nact=corpus_errors(d,f.read_text())
 if f.name not in readme: errs.append('README の表に無い')
 if errs:  # stderr: lint.sh は stdout を捨てるので、理由は stderr に出す
  failed+=1; print('FAIL corpus',f.name,file=sys.stderr); [print('  -',m,file=sys.stderr) for m in errs]
 else: print(f"PASS corpus {f.name}: {n} nodes, {nready}/{nact} actions ready, state={d['state']['readiness']}/{d['state']['execution_mode']}")
assert len(corpus)>=10,f'corpus は 10 本以上（今 {len(corpus)}）'
if failed: raise SystemExit(f'corpus: {failed}/{len(corpus)} fail')
print(f'PASS corpus {len(corpus)} files（静的検査のみ。描画・registry の実在・接続は未検査）')
