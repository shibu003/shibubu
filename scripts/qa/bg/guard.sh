#!/bin/bash
# 使い方: bash scripts/qa/bg/guard.sh "$HOME/…/guard.log" "$HOME/…/guard.stop" &   ← 背景の実機確認（cua-driver）の run の前に起こす
#   0.2 秒おきに前面の app（lsappinfo front の ASN）を log に書き、guard の起動後に起きた shibubu が前面を取ったらその pid を kill する（owner の操作を邪魔しない）
#   止める: touch "$HOME/…/guard.stop"。前面が変わった行は log の "front=" と "STOLE" で読む（docs/evidence/W2b/log.md 2026-10-20 の節で使った物）
#   窓の並び（src/main.rs W2b-fix2 の契約）: 毎回 oracle/ax.swift windows で画面の上の窓の並びを取り、起こした shibubu の窓が前面の app の窓より上に在れば
#   "OVER" を書いて kill。前面の app より下でも、shibubu 以外の窓が 1 枚でも自分の窓より後ろに在れば（一番後ろでない）"NOTBACK"
#   （直す前の build は前面の app の 1 つ下＝他の app 全部の上に並んだ・2026-10-20 00:51 の実測）。終わりに、起動時の前面の app の窓の位置と大きさを前後で比べ、変わっていれば "MOVED"。
#   STOLE / OVER / NOTBACK / MOVED が 1 つでもあれば最後の行は "RESULT fail" で exit 1（wait $! で受ける）。無ければ "RESULT pass"。
# guard.sh <log> <stopfile>: 0.2 秒おきに前面の ASN を記録。自分が起こした shibubu が前面になったら kill。
LOG=$1; STOP=$2
ts(){ perl -MTime::HiRes=time -MPOSIX=strftime -e '$t=time;printf "%s.%02d",strftime("%H:%M:%S",localtime $t),($t-int $t)*100'; }
name_of(){ lsappinfo list | awk -v a="$1" 'index($0,a){ if (match($0,/"[^"]*"/)) print substr($0,RSTART+1,RLENGTH-2); exit}'; }
pid_of_asn(){ lsappinfo list | awk -v a="$1" 'index($0,a){f=1} f&&/pid = /{sub(/.*pid = /,""); sub(/ .*/,""); print; exit}'; }
WIN=${TMPDIR:-/tmp}/shibubu-bg-windows
swiftc -O "$(dirname "$0")/oracle/ax.swift" -o "$WIN" || { echo "oracle/ax.swift の build に失敗" >&2; exit 2; }
fail=0
asn_of_pid(){ lsappinfo list | awk -v p="pid = $1 " '/ASN:/{a=$0; sub(/.*ASN:/,"ASN:",a); sub(/ .*/,"",a)} index($0,p){print a; exit}'; }
before=$(pgrep -x shibubu | sort | tr '\n' ' ')
prev=""; mine=""; masn=""
owner_pid=$(pid_of_asn "$(lsappinfo front)")
owner_before=$("$WIN" windows | awk -F'\t' -v p="$owner_pid" '$2==p{print $3"\t"$4}' | sort)
echo "$(ts) guard start; existing shibubu pids: [$before]; owner pid $owner_pid windows: $(echo $owner_before)" >> $LOG
while [ ! -e "$STOP" ]; do
  for p in $(pgrep -x shibubu); do case " $before $mine " in *" $p "*) ;; *) mine="$mine $p"; echo "$(ts) new shibubu pid $p" >> $LOG;; esac; done
  for p in $mine; do kill -0 $p 2>/dev/null || { mine=$(echo " $mine " | sed "s/ $p / /"); echo "$(ts) pid $p gone" >> $LOG; }; done
  f=$(lsappinfo front)
  if [ "$f" != "$prev" ]; then echo "$(ts) front=$f ($(name_of "$f"))" >> $LOG; prev=$f; fi
  cur=$(echo $mine | awk '{print $NF}')
  if [ -n "$cur" ] && [ "$cpid" != "$cur" ]; then masn=$(asn_of_pid $cur); [ -n "$masn" ] && { cpid=$cur; echo "$(ts) pid $cur asn=$masn" >> $LOG; }; fi
  if [ -n "$masn" ] && [ "$f" = "$masn" ]; then echo "$(ts) STOLE by $cpid -> kill" >> $LOG; fail=1; kill $cpid; fi
  if [ -n "$mine" ]; then
    fp=$(pid_of_asn "$f"); order=$("$WIN" windows)
    top=$(echo "$order" | awk -F'\t' -v p="$fp" '$2==p{print $1; exit}')
    over=$(echo "$order" | awk -F'\t' -v m=" $mine " -v t="${top:-999999}" 'index(m," "$2" ")&&$1<t{print $2; exit}')
    line="order front_pid=$fp top=${top:--} mine=$(echo "$order" | awk -F'\t' -v m=" $mine " 'index(m," "$2" "){printf "%s@%s ",$2,$1}')"
    [ "$line" != "$prevorder" ] && { echo "$(ts) $line" >> $LOG; prevorder=$line; }
    [ -n "$over" ] && { echo "$(ts) OVER by $over (above front pid $fp window #$top) -> kill" >> $LOG; fail=1; kill $over; }
    others=" $(pgrep -x shibubu | tr '\n' ' ') "
    notback=$(echo "$order" | awk -F'\t' -v m=" $mine " -v o="$others" 'index(m," "$2" ")&&!f{f=$2; next} f&&!index(o," "$2" "){print f" (behind it: "$5" #"$1")"; exit}')
    [ -n "$notback" ] && { echo "$(ts) NOTBACK by $notback -> kill" >> $LOG; fail=1; kill ${notback%% *}; }
  fi
  sleep 0.2
done
owner_after=$("$WIN" windows | awk -F'\t' -v p="$owner_pid" '$2==p{print $3"\t"$4}' | sort)
echo "$(ts) owner pid $owner_pid windows after: $(echo $owner_after)" >> $LOG
# run の間に owner が開け閉めした窓は比べない（前後の両方に在る窓 id だけ）
moved=$(awk -F'\t' 'NR==FNR{a[$1]=$2; next} ($1 in a)&&a[$1]!=$2{print $1": "a[$1]" -> "$2}' <(echo "$owner_before") <(echo "$owner_after"))
[ -n "$moved" ] && { echo "$(ts) MOVED $moved" >> $LOG; fail=1; }
echo "$(ts) guard stop" >> $LOG
[ $fail = 0 ] && { echo "$(ts) RESULT pass" >> $LOG; exit 0; }
echo "$(ts) RESULT fail" >> $LOG; exit 1
