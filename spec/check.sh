#!/bin/sh
# Model-checks every configuration in configs.txt and compares the result with the expected one:
# "pass" (no invariant violated) or the name of the invariant TLC must find violated.
# Usage: spec/check.sh [path/to/tla2tools.jar]
set -eu
cd "$(dirname "$0")"
JAR="${1:-${TLA2TOOLS:-$HOME/.cache/tla/tla2tools-1.7.4.jar}}"
out="${TMPDIR:-/tmp}/sw-tlc"; mkdir -p "$out"
fail=0
grep -v '^#' configs.txt | while read -r name lock cc ident refresh login expect invs; do
  [ -n "$name" ] || continue
  cfg="$out/$name.cfg"
  { echo "SPECIFICATION Spec"; echo "CONSTANTS"
    echo '  Accounts = {"a", "b"}'; echo '  Procs = {"p1", "p2"}'; echo '  MaxVer = 3'; echo '  MaxRetries = 2'
    echo "  UseSwLock = $lock"; echo "  HonorCcLock = $cc"; echo "  IdentityCheck = $ident"
    echo "  WithRefresh = $refresh"; echo "  WithLogin = $login"; echo "INVARIANTS"
    for i in $invs; do echo "  $i"; done; } > "$cfg"
  java -XX:+UseParallelGC -cp "$JAR" tlc2.TLC -config "$cfg" -workers auto -deadlock \
       -metadir "$out/$name.meta" SwSwitch.tla > "$out/$name.log" 2>&1 || true
  states=$(grep -oE '[0-9,]+ distinct states found' "$out/$name.log" | tail -1)
  if grep -q "Model checking completed. No error has been found" "$out/$name.log"; then got=pass
  else got=$(grep -oE 'Invariant [A-Za-z]+ is violated' "$out/$name.log" | head -1 | awk '{print $2}'); got=${got:-error}; fi
  if [ "$got" = "$expect" ]; then echo "ok    $name: $got ($states)"; else echo "FAIL  $name: expected $expect, got $got (see $out/$name.log)"; echo x >> "$out/.failed"; fi
done
[ ! -f "$out/.failed" ] || { rm -f "$out/.failed"; exit 1; }
