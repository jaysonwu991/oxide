#!/usr/bin/env python3
"""Read-only native helper smoke checks. Never requests OS grants or sends GUI input."""
import argparse
import json
import pathlib
import os
import tempfile
import select
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('app', type=pathlib.Path)
args = parser.parse_args()
app = args.app.resolve(strict=True)
command = app / 'Contents/MacOS/oxide-computer-use'
processes = []
scratch = tempfile.TemporaryDirectory(prefix="oxide-helper-policy-")
policy = pathlib.Path(scratch.name) / "policy.json"
policy.write_text(json.dumps({"denied_apps": ["Oxide Policy Check"]}))


def start():
    process = subprocess.Popen([str(command), '--mcp'], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               cwd=scratch.name, env={**os.environ, "COMPUTER_USE_CONFIG": "policy.json"})
    processes.append(process)
    return process


def request(process, identifier, method, params):
    process.stdin.write((json.dumps(dict(jsonrpc='2.0', id=identifier, method=method, params=params)) + '\n').encode())
    process.stdin.flush()
    if not select.select([process.stdout], [], [], 20)[0]:
        raise AssertionError('helper MCP response timed out')
    line = process.stdout.readline()
    if not line:
        raise AssertionError('helper disconnected: ' + process.stderr.read().decode())
    value = json.loads(line)
    assert value.get('id') == identifier and 'result' in value, value
    return value['result']


def initialize(process, identifier):
    result = request(process, identifier, 'initialize', {
        'protocolVersion': '2024-11-05', 'capabilities': {},
        'clientInfo': {'name': 'oxide-helper-check', 'version': '1'},
    })
    assert result['serverInfo']['name'] == 'computer-use'


def helper_processes():
    listing = subprocess.check_output(['/bin/ps', '-axo', 'pid=,command='], text=True)
    spellings = {str(app), str(app).replace('/private/tmp/', '/tmp/')}
    return [line for line in listing.splitlines()
            if any(path in line for path in spellings)
            and ('--serve' in line or '/Contents/Resources/computer-use' in line)]


try:
    assert not helper_processes(), 'close this development helper before testing'
    first = start()
    initialize(first, 101)
    assert request(first, 102, 'tools/list', {})['tools']
    listing = request(first, 103, 'tools/call', {'name': 'list_apps', 'arguments': {}})
    names = [line.split(' (pid ')[0] for line in listing['content'][0]['text'].splitlines() if ' (pid ' in line]
    assert names, 'native policy test needs a running windowed app (no app will be opened or controlled)'
    target = 'Oxide' if 'Oxide' in names else names[0]
    policy.write_text(json.dumps({'denied_apps': [target]}))
    second = start()
    initialize(second, 201)
    result = request(second, 202, 'tools/call', {'name': 'permissions', 'arguments': {}})
    assert not result.get('isError'), result
    permissions = json.loads(result['content'][0]['text'])
    assert pathlib.Path(permissions['launching_app']).resolve() == app, permissions
    blocked = request(second, 203, 'tools/call', {'name': 'get_app_state', 'arguments': {'app': target}})
    assert blocked.get('isError') and 'deny list' in blocked['content'][0]['text'], blocked
    print('PASS: two independent MCP connections, discovery, helper identity, relative custom deny policy')
    first.stdin.close()
    assert first.wait(timeout=5) == 0
    # SIGKILL skips all proxy cleanup: the helper watchdog must stop its runtime.
    second.kill()
    second.wait(timeout=5)
    deadline = time.monotonic() + 5
    while helper_processes() and time.monotonic() < deadline:
        time.sleep(0.1)
    assert not helper_processes(), 'helper or runtime survived transport loss'
    print('PASS: EOF cleanup and abrupt proxy-death cleanup; no helper or runtime left')
finally:
    for process in processes:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)

    scratch.cleanup()
