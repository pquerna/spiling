# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Exercise Cargo's real cache when historical source snapshots are restored."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

from loop import REPO, sync_source


class CargoCacheTests(unittest.TestCase):
    def test_changed_unchanged_and_restored_source_use_the_right_binary(self):
        channel = tomllib.loads((REPO / 'rust-toolchain.toml').read_text())['toolchain']['channel']
        cargo = ['rustup', 'run', channel, 'cargo']
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, build, target = root / 'source', root / 'build', root / 'target'
            (source / 'src').mkdir(parents=True)
            (source / 'Cargo.toml').write_text(
                '[package]\nname="route-cache-probe"\nversion="0.0.0"\nedition="2024"\n')
            (source / 'src/main.rs').write_text('fn main() {}\n')
            env = os.environ | {'CARGO_TARGET_DIR': str(target), 'CARGO_BUILD_JOBS': '1'}
            subprocess.run(cargo + ['generate-lockfile', '--offline', '--manifest-path',
                                   str(source / 'Cargo.toml')], env=env, check=True,
                           capture_output=True, text=True, timeout=60)
            receipts = []
            for value, changed in [('one', True), ('two', True), ('two', False), ('one', True)]:
                (source / 'src/main.rs').write_text(f'fn main() {{ println!("{value}"); }}\n')
                os.utime(source / 'src/main.rs', (1, 1))
                sync_source(source, build)
                run = subprocess.run(cargo + ['build', '--offline', '--locked', '--manifest-path',
                                     str(build / 'Cargo.toml'), '--message-format=json'],
                                     env=env, check=True, capture_output=True, text=True, timeout=60)
                artifacts = [json.loads(line) for line in run.stdout.splitlines()]
                binary = next(a for a in artifacts if a.get('executable'))
                self.assertEqual(binary['fresh'], not changed)
                result = subprocess.run([binary['executable']], check=True, capture_output=True,
                                        text=True, timeout=10)
                self.assertEqual(result.stdout.strip(), value)
                receipts.append((value, binary['fresh']))
            self.assertEqual(receipts, [('one', False), ('two', False), ('two', True), ('one', False)])


if __name__ == '__main__':
    unittest.main()
