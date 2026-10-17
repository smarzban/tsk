"""Real CLI TTY gating, fake Herdr only. Opt in with TSK_TEST_BINARY after a build."""
import errno
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time
import unittest


@unittest.skipUnless(os.name == "posix" and os.environ.get("TSK_TEST_BINARY"), "requires POSIX and TSK_TEST_BINARY")
class SetupTtyTests(unittest.TestCase):
    def test_conflict_yes_no_and_piped_yes(self):
        import pty
        binary = str(Path(os.environ["TSK_TEST_BINARY"]).resolve())
        for answer, piped in [(b"y\n", False), (b"n\n", False), (b"y\n", True)]:
            with self.subTest(answer=answer, piped=piped), tempfile.TemporaryDirectory(prefix="tsk-tty-") as tmp:
                root = Path(tmp)
                (root / "bin").mkdir()
                (root / "config").mkdir()
                config = root / "config/config.toml"
                source = "[keys]\nnew_tab = 'prefix+t'\n"
                config.write_text(source)
                host = root / "bin/herdr"
                host.write_text(f"#!{sys.executable}\n" + '''import json, os, pathlib, sys
root = pathlib.Path(os.environ["SETUP_FIXTURE"])
args = sys.argv[1:]
with (root / "calls").open("a") as log: log.write(json.dumps(args) + "\\n")
registry = root / "registry"
if args == ["--version"]:
    print("herdr 0.9.0")
if args[:2] == ["plugin", "link"]:
    registry.write_text(json.dumps({"result":{"plugins":[{"plugin_id":"herdr-tsk","plugin_root":args[2]}]}}))
if args[:2] == ["plugin", "list"]:
    print(registry.read_text() if registry.exists() else '{"result":{"plugins":[]}}')
''')
                host.chmod(0o755)
                environment = dict(os.environ, PATH=f"{root / 'bin'}:{os.environ['PATH']}", SETUP_FIXTURE=str(root), HERDR_CONFIG_PATH=str(config), HOME=str(root / "home"), XDG_CONFIG_HOME=str(root / "xdg"), XDG_STATE_HOME=str(root / "state"), HERDR_SOCKET_PATH=str(root / "absent.sock"), TSK_STATE_DIR=str(root / "tasks"))
                master, slave = pty.openpty()
                process = None
                transcript = b""
                try:
                    process = subprocess.Popen([binary, "setup", "herdr"], stdin=subprocess.PIPE if piped else slave, stdout=slave, stderr=slave, env=environment, start_new_session=True)
                    os.close(slave)
                    slave = None
                    if piped:
                        # tsk refuses a piped stdin without reading it and may already have
                        # exited; the flush on close then raises. The assertions below
                        # still prove the refusal.
                        try:
                            process.stdin.write(answer)
                            process.stdin.close()
                        except BrokenPipeError:
                            pass
                    replied = piped
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        if not select.select([master], [], [], 0.1)[0]:
                            if process.poll() is not None: break
                            continue
                        try:
                            chunk = os.read(master, 8192)
                        except OSError as error:
                            if error.errno == errno.EIO: break
                            raise
                        if not chunk: break
                        transcript += chunk
                        if b"[y/N]" in transcript and not replied:
                            os.write(master, answer)
                            replied = True
                    code = process.wait(timeout=2)
                    output = transcript.decode(errors="replace")
                    if piped:
                        self.assertNotEqual(code, 0, output)
                        self.assertNotIn("[y/N]", output)
                        self.assertEqual(config.read_text(), source)
                        self.assertFalse((root / "calls").exists())
                        self.assertEqual(list(config.parent.iterdir()), [config])
                    else:
                        self.assertTrue(replied, output)
                        self.assertEqual(code, 0, output)
                        updated = config.read_text()
                        self.assertEqual("new_tab" in updated, answer == b"n\n")
                        self.assertEqual("herdr-tsk.open-board" in updated, answer == b"y\n")
                        self.assertIn("herdr-tsk.quick-capture", updated)
                        calls = [json.loads(line) for line in (root / "calls").read_text().splitlines()]
                        self.assertTrue(any(call[:2] == ["plugin", "link"] for call in calls))
                finally:
                    if process is not None and process.poll() is None:
                        process.kill()
                        process.wait()
                    if slave is not None: os.close(slave)
                    os.close(master)
