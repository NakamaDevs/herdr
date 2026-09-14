#!/usr/bin/env python3
"""Dependency-free Kaicho 2.1 delivery checks; no credential or network access."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
TYPES = 'feat|fix|docs|refactor|test|build|ci|chore|perf|style|revert'

def message_error(text):
    lines = text.splitlines()
    if not lines or len(lines[0]) > 100 or not re.fullmatch(rf'({TYPES})(\([a-z0-9][a-z0-9-]*\))?!?: \S.*', lines[0]):
        return 'Use a Conventional Commit header, at most 100 characters.'
    if not re.search(r'(?im)^(Refs|Fixes) (NAK|OPS|APP|GMD)-[1-9][0-9]*\b', '\n'.join(lines[1:])):
        return 'Include Refs TEAM-123 or Fixes TEAM-123 in the commit body.'
    return None

def branch_error(branch, profile):
    if branch in [profile['default_branch'], *profile.get('existing_branches', [])]:
        return None
    if re.fullmatch(rf'({TYPES}|feature)/(NAK|OPS|APP|GMD)-[1-9][0-9]*-[a-z0-9][a-z0-9-]*', branch):
        return None
    return 'Use type/TEAM-123-description. Existing published branches remain valid.'

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('command',choices=['commit','branch','self-test'])
    p.add_argument('message_file',nargs='?')
    p.add_argument('--format-only',action='store_true')
    args=p.parse_args()
    if args.command=='self-test':
        for team in ['NAK','OPS','APP','GMD']:
            assert message_error(f'fix: retain draft\n\nRefs {team}-946') is None
            assert branch_error(f'fix/{team}-946-delivery',{'default_branch':'main'}) is None
        for text in ['', 'fix: missing issue', 'NAK-946: wrong header', 'fix: missing word\n\nNAK-946']:
            assert message_error(text)
        assert branch_error('fix/cx/NAK-946-new',{'default_branch':'main'})
        assert branch_error('old/OPS-1',{'default_branch':'main','existing_branches':['old/OPS-1']}) is None
        print('Kaicho 2.1 delivery tests passed.');return 0
    if args.command=='commit':
        if not args.message_file:p.error('commit requires a message file')
        error=message_error(Path(args.message_file).read_text(encoding='utf-8-sig'))
    else:
        profile=json.loads((ROOT/'.nakama/delivery.json').read_text())
        branch=subprocess.check_output(['git','-C',str(ROOT),'branch','--show-current'],text=True).strip()
        error=branch_error(branch,profile)
    if error:print(error,file=sys.stderr);return 1
    return 0

if __name__=='__main__':raise SystemExit(main())
