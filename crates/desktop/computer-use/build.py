#!/usr/bin/env python3
"""Build a standalone helper from an explicitly supplied Computer Use runtime."""
import argparse
import pathlib
import plistlib
import platform
import tempfile
import shutil
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--server', type=pathlib.Path, required=True, help='Built computer-use MCP executable')
parser.add_argument('--output', type=pathlib.Path, required=True, help='New .app destination')
parser.add_argument('--identity', default='-', help='Developer ID signing identity; default is ad-hoc for development')
args = parser.parse_args()
server = args.server.resolve(strict=True)
output = args.output.absolute()
if output.suffix != '.app' or output.exists():
    parser.error('--output must be a new .app path (existing bundles are never overwritten)')
source = pathlib.Path(__file__).resolve().parent
contents = output / 'Contents'
macos = contents / 'MacOS'
resources = contents / 'Resources'
macos.mkdir(parents=True)
resources.mkdir()
with tempfile.TemporaryDirectory(prefix='oxide-swift-') as cache:
    subprocess.run(['xcrun', 'swiftc', '-O', '-module-cache-path', cache,
                    '-target', f'{platform.machine()}-apple-macosx13.0',
                    str(source / 'main.swift'), '-o', str(macos / 'oxide-computer-use')], check=True)
shutil.copy2(server, resources / 'computer-use')
(resources / 'computer-use').chmod(0o755)
shutil.copy2(source.parent / 'icons/icon.icns', resources / 'icon.icns')
with (contents / 'Info.plist').open('wb') as file:
    plistlib.dump({
        'CFBundleIdentifier': 'dev.oxide.computer-use',
        'CFBundleName': 'Oxide Computer Use',
        'CFBundleDisplayName': 'Oxide Computer Use',
        'CFBundleExecutable': 'oxide-computer-use',
        'CFBundlePackageType': 'APPL',
        'CFBundleVersion': '1',
        'CFBundleShortVersionString': '0.1.0',
        'CFBundleIconFile': 'icon.icns',
        'LSUIElement': True,
        'LSMinimumSystemVersion': '14.0',
        'NSHighResolutionCapable': True,
    }, file)
for target in [resources / 'computer-use', output]:
    subprocess.run(['codesign', '--force', '--options', 'runtime', '--timestamp=none' if args.identity == '-' else '--timestamp',
                    '--sign', args.identity, str(target)], check=True)
subprocess.run(['codesign', '--verify', '--deep', '--strict', str(output)], check=True)
print(f'Built {output}')
print(f'MCP command: {macos / "oxide-computer-use"} --mcp')
