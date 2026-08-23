#!/usr/bin/env bash
# Record wake-word training samples for "Five" (rustpotter .rpw training).
# Each take is a 2-second 16kHz mono WAV. Positives go in the top level,
# negatives in negative/. Re-runnable: takes are numbered, existing files
# are skipped unless deleted first.
set -u
cd "$(dirname "$0")/.."
BIN=./target/debug/five-daemon
CFG=config.dev.yaml
OUT=models/wakeword-samples
mkdir -p "$OUT/negative"

take() { # take <path> <cue>
  local path="$1" cue="$2"
  if [ -f "$path" ]; then echo "   (skip, exists: $path)"; return; fi
  echo ""
  echo "→ $cue"
  read -r -p "   press Enter, then speak right away " _
  "$BIN" --config "$CFG" record -o "$path" -d 2 >/dev/null 2>&1 \
    && echo "   ✓ saved $path" || echo "   ✗ FAILED $path"
}

echo "=== Five wake-word sample recording ==="
echo "16 positive takes of 'Five', then 6 negatives."
echo "Mic: Blue Yeti Nano (hw:2,0). Keep ~1-2 ft away, cardioid pattern."

take "$OUT/five-01.wav" "Say: Five (normal, relaxed)"
take "$OUT/five-02.wav" "Say: Five (normal again)"
take "$OUT/five-03.wav" "Say: Five (a bit slower)"
take "$OUT/five-04.wav" "Say: Five (a bit quicker)"
take "$OUT/five-05.wav" "Ask it: Five? (rising intonation)"
take "$OUT/five-06.wav" "Ask it again: Five?"
take "$OUT/five-07.wav" "Say: Five (slightly louder)"
take "$OUT/five-08.wav" "Say: Five (soft, almost a murmur)"
take "$OUT/five-09.wav" "Say: Five (stretch it: Fiiive)"
take "$OUT/five-10.wav" "Say: Five (crisp and short)"
take "$OUT/five-11.wav" "Take a step back, say: Five"
take "$OUT/five-12.wav" "Two steps back, say: Five (project a little)"
take "$OUT/five-13.wav" "Back at the mic: Five"
take "$OUT/five-14.wav" "Say: Five (tired/morning voice)"
take "$OUT/five-15.wav" "Say: Five (smile while saying it)"
take "$OUT/five-16.wav" "One more, your call how: Five"

echo ""
echo "=== Negatives (things that must NOT trigger) ==="
take "$OUT/negative/neg-01.wav" "Stay silent (room tone only)"
take "$OUT/negative/neg-02.wav" "Say a full sentence, e.g. 'what time is it today'"
take "$OUT/negative/neg-03.wav" "Say: hey there"
take "$OUT/negative/neg-04.wav" "Say: fifty-five (near-miss word)"
take "$OUT/negative/neg-05.wav" "Say: I've got five minutes (word in context)"
take "$OUT/negative/neg-06.wav" "Type on the keyboard / clear your throat"

echo ""
echo "Done. Samples in $OUT/"
ls -1 "$OUT"/*.wav "$OUT"/negative/*.wav 2>/dev/null | wc -l
