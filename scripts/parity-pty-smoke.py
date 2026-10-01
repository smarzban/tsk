#!/usr/bin/env python3
"""Real binary in an isolated PTY. Records ANSI output, not GUI screenshots."""
import fcntl
import hashlib
import json
import os
import pathlib
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
import tomllib

import pyte

repo = pathlib.Path(__file__).resolve().parents[1]
output = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else repo / 'site/parity-reference/pty'
output.mkdir(parents=True, exist_ok=True)
binary = repo / 'target/release/tsk'
fixture = repo / 'tests/fixtures/demo-parity/store.json'

for width in (40, 78, 109, 110):
    with tempfile.TemporaryDirectory(prefix='tsk-parity-') as scratch:
        scratch = pathlib.Path(scratch)
        state = scratch / 'state'
        state.mkdir()
        project = scratch / 'tsk-parity'
        (project / '.git').mkdir(parents=True)
        document = json.loads(fixture.read_text())
        for task in document['tasks']:
            if isinstance(task['scope'], dict):
                task['scope']['project']['path'] = str(project)
        (state / 'tsk.json').write_text(json.dumps(document))
        # Keep this task-only fixture stable as guide and announcement catalogs grow.
        guide_ids = re.findall(r'catalog_id: "([^"]+)"', (repo / 'src/guides.rs').read_text())
        announcements = tomllib.loads((repo / 'src/announcements/catalog.toml').read_text())
        # A comment-only catalog means nothing to announce; the watermark stays at 0.
        newest_announcement = max((entry['id'] for entry in announcements.get('announcement', [])), default=0)
        (state / 'delivery.json').write_text(json.dumps({
            'guides': guide_ids,
            'announcement_watermark': newest_announcement,
        }))
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, width, 0, 0))
        env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'TSK_'))}
        env.update(TSK_STATE_DIR=str(state), TERM='xterm-256color')
        child = subprocess.Popen(
            [str(binary)], stdin=slave, stdout=slave, stderr=slave, cwd=project,
            env=env, start_new_session=True,
            preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
        )
        os.close(slave)
        transcript = bytearray()
        screen = pyte.Screen(width, 24)
        stream = pyte.ByteStream(screen)

        def drain():
            data = bytearray()
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.15)
                if ready:
                    try:
                        data.extend(os.read(master, 65536))
                    except OSError:
                        break
                elif data:
                    break
            transcript.extend(data)
            stream.feed(bytes(data))
            return bytes(data)

        def key(value, name):
            os.write(master, value)
            drain()
            (output / f'pty-{width}-{name}.ansi').write_bytes(transcript)
            (output / f'pty-{width}-{name}.txt').write_text('\n'.join(screen.display))

        def saved_task(number, ready):
            # A settled screen does not mean the save has landed: the board repaints
            # before it persists, and wide layouts repaint in several chunks. Poll.
            deadline = time.monotonic() + 3
            while True:
                saved = json.loads((state / 'tsk.json').read_text())
                task = next(t for t in saved['tasks'] if t['number'] == number)
                if ready(task) or time.monotonic() >= deadline:
                    return task
                time.sleep(0.05)

        try:
            first = drain()
            deadline = time.monotonic() + 8
            while b'NEEDS YOU' not in first and time.monotonic() < deadline:
                first += drain()
            assert b'NEEDS YOU' in first, repr(first)
            key(b'1', 'initial')
            assert 'ON DECK' in '\n'.join(screen.display)
            key(b'\x1b[C', 'peek-or-split')
            if width < 110:
                assert '└─ #release · tsk-parity' in '\n'.join(screen.display)
            key(b'\x1b[D', 'closed')
            key(b'\x1b[B', 'selected')
            key(b'\x13', 'started')
            assert saved_task(13, lambda t: t['status'] == 'started')['status'] == 'started'
            key(b'\r', 'page')
            assert 'plain note' in '\n'.join(screen.display)
            key(b'\t', 'step-selected')
            key(b'\r', 'step-toggled')
            task = saved_task(13, lambda t: t['steps'][0]['done'])
            assert task['steps'][0]['done'] and task['status'] == 'started'
            key(b'\x01', 'step-add-editor')
            key(b'New step\r', 'step-added')
            assert len(saved_task(13, lambda t: len(t['steps']) == 3)['steps']) == 3
            key(b'\x1b', 'step-add-canceled')
            key(b'\x1b', 'back')
            assert 'IN MOTION' in '\n'.join(screen.display)
            key(b'\x11', 'quit')
            child.wait(timeout=5)
            assert child.returncode == 0
            print(f'{width}x24: initial, peek/split, select, persisted start, page, persisted step toggle/add, back, clean quit OK')
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            os.close(master)

(output / 'provenance.json').write_text(json.dumps({
    'kind': 'real PTY ANSI transcript and decoded screen; not screenshot',
    'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
    'binary': str(binary),
    'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
    'fixture_sha256': hashlib.sha256(fixture.read_bytes()).hexdigest(),
    'sizes': [[w, 24] for w in (40, 78, 109, 110)],
    'clock': 'real system clock; timestamps differ from fixed browser clock',
    'scope': 'fixture project path relocated to temporary tsk-parity project; key 1 opens desk',
}, indent=2) + '\n')
