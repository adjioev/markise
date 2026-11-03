#!/usr/bin/env python3
"""Measure per-test process wall time, separately from Cargo build time (stdlib only).

Run from the workspace root. Examples:
  python3 scripts/test-times.py --match adversarial --output /tmp/before.json
  python3 scripts/test-times.py --release --heavy --output /tmp/heavy.json
No budget assertions: these are local observations, not CI performance measurements.
"""
import argparse
import json
from pathlib import Path
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--release', action='store_true')
parser.add_argument('--heavy', action='store_true', help='only heavy_suite_ ignored tests; excludes known bugs')
parser.add_argument('--match', action='append', default=[], help='test name substring (repeatable)')
parser.add_argument('--output', required=True)
args = parser.parse_args()
command = ['cargo', 'test', '--workspace', '--locked', '--no-run', '--message-format=json']
if args.release:
    command.append('--release')
started = time.monotonic()
build = subprocess.run(command, stdout=subprocess.PIPE, text=True, check=True)
report = {'build_seconds': time.monotonic() - started, 'profile': 'release' if args.release else 'debug',
          'note': 'Per-test wall time includes process startup; isolated tests, not parallel suite time.', 'tests': []}
executables = set()
for line in build.stdout.splitlines():
    message = json.loads(line)
    if message.get('reason') == 'compiler-artifact' and message.get('profile', {}).get('test') and message.get('executable'):
        executables.add(message['executable'])
try:
    for executable in sorted(executables):
        listed = subprocess.check_output([executable, '--list', '--format=terse'] + (['--ignored'] if args.heavy else []), text=True)
        for line in listed.splitlines():
            if not line.endswith(': test'):
                continue
            name = line[:-6]
            if args.heavy != ('heavy_suite_' in name) or (args.match and not any(m in name for m in args.match)):
                continue
            # Ordinary listing includes known-bug ignores; verify selected tests actually run.
            command = [executable, name, '--exact', '--nocapture'] + (['--ignored'] if args.heavy else [])
            started = time.monotonic()
            result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            seconds = time.monotonic() - started
            if '0 passed; 0 failed; 1 ignored' in result.stdout:
                continue
            row = {'binary': Path(executable).name, 'name': name, 'seconds': seconds, 'exit_code': result.returncode}
            report['tests'].append(row)
            print(f'{seconds:8.3f}s {name} exit={result.returncode}', flush=True)
            if result.returncode:
                print(result.stdout, flush=True)
finally:
    Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
if any(t['exit_code'] for t in report['tests']):
    raise SystemExit(1)
