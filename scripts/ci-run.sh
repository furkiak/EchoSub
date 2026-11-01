#!/usr/bin/env bash
# CI yardımcısı: komutu çalıştırır. Başarısız olursa hata satırlarını GitHub "annotation"
# olarak da yazar; böylece giriş yapmadan (API üzerinden) hata nedeni okunabilir.
# Kullanım: scripts/ci-run.sh <komut> [argümanlar...]
set -o pipefail
log="$(mktemp)"
"$@" 2>&1 | tee "$log"
code=${PIPESTATUS[0]}
if [ "$code" -ne 0 ]; then
  errs="$(grep -nE '^(error|Error)|panicked at|undefined reference|cannot find|linker|ld: |LLVM ERROR|fatal' -A6 "$log" | head -c 3000)"
  tail_="$(tail -n 25 "$log" | head -c 2500)"
  msg="[$*] çıkış kodu $code"$'\n'"--- hata satırları ---"$'\n'"$errs"$'\n'"--- son satırlar ---"$'\n'"$tail_"
  msg="${msg//'%'/'%25'}"; msg="${msg//$'\r'/}"; msg="${msg//$'\n'/'%0A'}"
  echo "::error title=Komut başarısız::$msg"
fi
exit "$code"
