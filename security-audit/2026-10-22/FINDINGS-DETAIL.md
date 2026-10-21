# Findings detail

One confirmed finding, severity medium.

## A bare https token stays in the backup when the URL does not start at the scheme

Fingerprint: `backup.https-bare-userinfo.scheme-prefix`

### Trace

1. Entrypoint. `src/cli/backup.rs` `backup_add` (line 487). A location that is not an existing directory is stored as written.
2. Propagation. `src/backup.rs` `absolute_path` (line 431) returns `None` for a git URL.
3. Propagation. `src/backup.rs` `add_target_locked` (line 334) writes that string as the backup `url`.
4. Propagation. `src/backup.rs` `run` (line 919) passes `config.toml` through `sanitized_config`.
5. Propagation. `src/backup.rs` `sanitized_config` (line 1059) rewrites a url only when `without_credentials` returns `Some`.
6. Propagation. `src/backup.rs` `credentials` (line 1140) takes the scheme as every byte before `://`. Line 1146 returns `None` when that prefix is not `http` or `https` and the userinfo has no colon.
7. Propagation. `src/backup.rs` `without_credentials` (line 1079) returns `None`, so the url stays in the document.
8. Sink. `src/backup.rs` `backup_to_git` (line 1244) writes those bytes to `machines/<id>/config.toml` (line 1197) and `git push` publishes the commit.

`shown_location` (line 1090) prints the same unstripped string. `redacted` (line 1104) masks it.

### Parties

The operator stored the URL. The lower-trust reader is anyone who can read the backup repository. The affected resource is the bare token in the pushed `config.toml`.

### Local check

Payloads:

- `https://dummy-token@example.test/repo.git`
- ` https://dummy-token@example.test/repo.git`
- a leading newline, then `https://dummy-token@example.test/repo.git`

The harness copied `without_credentials`, `shown_location`, `redacted`, and `credentials` from `src/backup.rs` and ran in a user and network namespace with an empty environment, `prlimit`, and `timeout`. No remote was contacted.

Observed:

```text
clean
without=Some("https://example.test/repo.git")
shown="https://***@example.test/repo.git"
redacted="https://***@example.test/repo.git"
space
without=None
shown=" https://dummy-token@example.test/repo.git"
redacted=" https://***@example.test/repo.git"
newline
without=None
shown="\nhttps://dummy-token@example.test/repo.git"
redacted="\nhttps://***@example.test/repo.git"
```

The invariant this proves: a bare https userinfo is a credential even when bytes sit before the scheme. The clean URL already meets it. The prefixed URLs do not.

### Conditions

The url does not begin at the scheme, and its userinfo has no colon. `tsk backup add` and `parse_target` keep those bytes. A later backup pushes that `config.toml`. A `user:password` userinfo is still stripped.

### Remediation

Classify `http` and `https` from the trailing scheme token, the same scan `redacted()` uses. Add a regression test that `sanitized_config` and `shown_location` drop `dummy-token` for a leading space and a leading newline.

Share that scan in one function so the push path and the display path cannot drift again.
