# Records docs/demo.gif with https://github.com/charmbracelet/vhs: `vhs docs/demo.tape`.
# It expects the small example project from docs/demo-project.sh in /tmp/lspyx-demo.
Output docs/demo.gif

Set Shell "bash"
Set FontSize 20
Set Width 1200
Set Height 760
Set Padding 28
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set PlaybackSpeed 1.0

Hide
Type "cd /tmp/lspyx-demo && export PS1='$ ' && lspyx daemon ensure >/dev/null && clear"
Enter
Show

Type "lspyx find-symbol Cart"
Sleep 400ms
Enter
Sleep 2.5s

Type "lspyx usages src/shop/models.py:15:9"
Sleep 400ms
Enter
Sleep 3.5s

Hide
Type "clear"
Enter
Show

Type "lspyx outline src/shop/models.py"
Sleep 400ms
Enter
Sleep 3s

Type "lspyx goto src/shop/checkout.py:14:12"
Sleep 400ms
Enter
Sleep 4s
