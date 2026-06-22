# Demo recording script for tare. Generate the GIF with:  vhs docs/assets/demo.tape
# (install vhs: https://github.com/charmbracelet/vhs)
Output docs/assets/demo.gif

Set FontSize 18
Set Width 1200
Set Height 640
Set Theme "Dracula"
Set Padding 20

Type "# tare: skeletonize a source file — keep signatures, drop bodies"
Enter
Sleep 800ms
Type "tare skeletonize --path crates/tare-core/src/code_skeleton.rs \"
Enter
Type "  < crates/tare-core/src/code_skeleton.rs | head -25"
Enter
Sleep 3s

Type "# tare: compact ps aux (cap rows + truncate the command column)"
Enter
Sleep 800ms
Type "ps aux | tare compact-lossy --max-rows 20 --max-field 90 | head -12"
Enter
Sleep 3s

Type "# run it as a transparent proxy — point your agent at http://localhost:8787"
Enter
Sleep 800ms
Type "TARE_UPSTREAM=https://api.anthropic.com tare-proxy"
Enter
Sleep 2s
