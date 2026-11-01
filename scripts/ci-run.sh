#!/usr/bin/env bash
# CI yardımcısı: komutu çalıştırır. Başarısız olursa hata satırlarını GitHub "annotation"
# olarak da yazar; böylece giriş yapmadan (API üzerinden) hata nedeni okunabilir.
# Kullanım: scripts/ci-run.sh <komut> [argümanlar...]
set -o pipefail
log="$(mktemp)"
"$@" 2>&1 | tee "$log"
code=${PIPESTATUS[0]}
if [ "$code" -ne 0 ]; then
  clean="$(mktemp)"; grep -v 'ld: warning' "$log" > "$clean"   # uyarı gürültüsünü ayıkla
  errs="$(grep -nE '^(error|Error)|error:|panicked at|undefined (reference|symbol)|cannot find|LLVM ERROR|fatal' -A4 "$clean" | head -c 3000)"
  tail_="$(tail -n 25 "$clean" | head -c 2500)"
  msg="[$*] çıkış kodu $code"$'\n'"--- hata satırları ---"$'\n'"$errs"$'\n'"--- son satırlar ---"$'\n'"$tail_"
  msg="${msg//'%'/'%25'}"; msg="${msg//$'\r'/}"; msg="${msg//$'\n'/'%0A'}"
  echo "::error title=Komut başarısız::$msg"
fi
exit "$code"
