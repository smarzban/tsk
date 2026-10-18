# Clip 5: the dispatch loop (~40s). render.sh prepends the header (size, font, theme, output).
#
# Captions: while recording is hidden, prefix+j focuses the caption strip under the board,
# the caption is typed with Enter, and prefix+; (last_pane) focuses back, so each caption
# change is a cut. prefix+] and prefix+[ visit the agent's workspace and come back.

# Attach to the clip's own Herdr server; the seed left the board open in the api workspace.
Hide
Type "clear && exec herdr"
Enter
Wait+Screen /ON DECK/
Sleep 1.5s
Ctrl+B
Type "j"
Type@5ms "@ assigns a task to an agent profile"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 1.5s
Down
Sleep 1s
Type "@"
Sleep 1.5s
Enter
Sleep 1.5s

Hide
Ctrl+B
Type "j"
Type@5ms "ctrl+s dispatches: a worktree, a workspace, the agent"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 1.5s
Ctrl+S
Sleep 2s

# The agent's own workspace, while it works.
Ctrl+B
Type "]"
Wait+Screen /waiting for review/
Sleep 1.5s

Hide
Ctrl+B
Type "["
Sleep 300ms
Ctrl+B
Type "j"
Type@5ms "It hands back: what it did, and what to check"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 3s

Hide
Ctrl+B
Type "j"
Type@5ms "Open it and tick the checks"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 1s
Enter
Sleep 1.5s
Enter
Sleep 1s
Tab
Sleep 600ms
Enter
Sleep 1.5s

Hide
Ctrl+B
Type "j"
Type@5ms "ctrl+d approves · m merges the PR and cleans up"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 1s
Ctrl+D
Sleep 2.5s
Type "m"
Wait+Screen /cleaned/
Sleep 2.5s

Hide
Ctrl+B
Type "j"
Type@5ms "Merged and done; worktree, branch, workspace gone"
Enter
Ctrl+B
Type ";"
Sleep 500ms
Show

Sleep 1s
Escape
Sleep 1s
Type "d"
Sleep 3s
