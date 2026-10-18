# Clip 1: quick capture (~20s). render.sh prepends the header (size, font, theme, output).
#
# Captions: while recording is hidden, prefix+j focuses the caption strip under the work,
# the caption is typed with Enter, and prefix+; (last_pane) focuses back, so each caption change is a cut.

# Attach to the clip's own Herdr server (render.sh started it; HERDR_SOCKET_PATH points there).
Hide
Type "clear && exec herdr"
Enter
Wait+Screen /api ❯/
Sleep 1.5s
Ctrl+B
Type "j"
Type@5ms "prefix+t opens the board beside your work"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 2s
Ctrl+B
Sleep 200ms
Type "t"
Wait+Screen /INBOX|inbox/
Sleep 2s

Hide
Ctrl+B
Type "j"
Type@5ms "+ adds a task · !p sets its project, !t its thread"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 1.5s
Type "+"
Sleep 800ms
Type "Fix login timeout !p api !t auth"
Sleep 1.2s
Enter
Sleep 1.5s

Hide
Ctrl+B
Type "j"
Type@5ms "It lands in the inbox with its thread"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 2.5s

Hide
Ctrl+B
Type "j"
Type@5ms "ctrl+n makes it ready: on deck"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 1.5s
Ctrl+N
Sleep 3s
