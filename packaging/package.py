#!/usr/bin/env python3
# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
"""Build and pack the release downloads for Windows, Linux and macOS.

The release workflow (.github/workflows/release.yml) runs this on each system; it also runs on a desktop.
Everything about the product (its name, program name, crate folder and documents) comes from product.json
beside this file, so the same script serves every whispr.dev program.

    package.py env                       print NAME=, BINARY=, VERSION= and CRATE_DIR= lines
    package.py build --platform P        cargo build for platform P, leaving the program in dist/bin/
    package.py pack --platform P         pack dist/bin/'s program into dist/out/
    package.py release-files --tag vX    add version-less copies of the downloads, then write
                                         dist/out/SHA256SUMS.txt and the release notes, dist/notes.md

Platforms, and what `pack` makes for each (all names start with <binary>-<version>-<platform>):
    windows-x86_64    -setup.exe (an installer that shows the licence) and .zip
    linux-x86_64      .run (a self-extracting installer that shows the licence) and .tar.gz
    linux-aarch64     .run and .tar.gz
    macos-universal   .run and .tar.gz (one program for Apple silicon and Intel Macs)

`release-files` also copies each download to a name without the version (<binary>-<platform>...), so that
https://github.com/OWNER/REPO/releases/latest/download/<that name> always gives the newest release.

The installers ask the user to accept the licence, then ask which folder to install into, and change nothing
else: no registry entries, no PATH changes and no uninstaller. Removing the program means deleting its folder.
The Windows installer needs Inno Setup's ISCC.exe (--iscc, or the ISCC environment variable); the others need
makeself.sh (--makeself, or MAKESELF). Needs Python 3.8 or newer and only the standard library.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
DIST = ROOT / 'dist'
# The date stamped on every archive entry, so the same program always packs the same way. Zip can't go
# before 1980.
FIXED_DATE = (1980, 1, 1, 0, 0, 0)
FIXED_EPOCH = 315532800
PLATFORMS = {
    'windows-x86_64': ['x86_64-pc-windows-msvc'],
    'linux-x86_64': ['x86_64-unknown-linux-musl'],
    'linux-aarch64': ['aarch64-unknown-linux-musl'],
    'macos-universal': ['aarch64-apple-darwin', 'x86_64-apple-darwin'],
}
# The oldest macOS each half of the universal program runs on.
MACOS_MINIMUM = {'aarch64-apple-darwin': '11.0', 'x86_64-apple-darwin': '10.13'}


class Fail(Exception):
    """A problem worth one line on stderr and exit code 1."""


def product() -> dict:
    data = json.loads((HERE / 'product.json').read_text(encoding='utf-8'))
    for key in ('name', 'binary', 'crate_dir', 'url', 'download_page', 'docs'):
        if key not in data:
            raise Fail(f'packaging/product.json has no "{key}"')
    data['crate'] = (ROOT / data['crate_dir']).resolve()
    data['version'] = crate_version(data['crate'])
    return data


def crate_version(crate: Path) -> str:
    """The `version` line of the [package] table in the crate's Cargo.toml."""
    text = (crate / 'Cargo.toml').read_text(encoding='utf-8')
    package = re.search(r'^\[package\]\s*$(.*?)(?=^\[|\Z)', text, re.MULTILINE | re.DOTALL)
    version = package and re.search(r'^version\s*=\s*"([^"]+)"', package.group(1), re.MULTILINE)
    if not version:
        raise Fail(f'cannot find the package version in {crate / "Cargo.toml"}')
    return version.group(1)


def program_name(info: dict, platform: str) -> str:
    return info['binary'] + ('.exe' if platform.startswith('windows') else '')


def stem(info: dict, platform: str) -> str:
    return f"{info['binary']}-{info['version']}-{platform}"


def run(command: list, **kwargs) -> None:
    print('+ ' + ' '.join(str(part) for part in command), flush=True)
    result = subprocess.run([str(part) for part in command], check=False, **kwargs)
    if result.returncode != 0:
        raise Fail(f'{Path(str(command[0])).name} failed with exit code {result.returncode}')


# --------------------------------------------------------------------------------------------------------------
# build
# --------------------------------------------------------------------------------------------------------------

def target_dir(info: dict) -> Path:
    configured = os.environ.get('CARGO_TARGET_DIR')
    return Path(configured).resolve() if configured else info['crate'] / 'target'


def build(info: dict, platform: str, targets: list) -> Path:
    built = []
    for target in targets:
        env = dict(os.environ)
        if target.endswith('-linux-musl'):
            # musl-tools' compiler, for the C parts (bundled SQLite, ring).
            env.setdefault('CC_' + target.replace('-', '_'), 'musl-gcc')
        if target in MACOS_MINIMUM:
            env['MACOSX_DEPLOYMENT_TARGET'] = MACOS_MINIMUM[target]
        run(['cargo', 'build', '--locked', '--release', '--target', target], cwd=info['crate'], env=env)
        program = target_dir(info) / target / 'release' / program_name(info, platform)
        if not program.is_file():
            raise Fail(f'cargo finished but {program} is missing')
        built.append(program)
    out = DIST / 'bin'
    out.mkdir(parents=True, exist_ok=True)
    final = out / program_name(info, platform)
    if len(built) == 1:
        shutil.copy2(built[0], final)
    else:
        # One program that runs on both Apple silicon and Intel Macs.
        run(['lipo', '-create', '-output', final, *built])
    if not platform.startswith('windows'):
        final.chmod(0o755)
    print(f'Built {final} ({final.stat().st_size:,} bytes)')
    return final


# --------------------------------------------------------------------------------------------------------------
# texts shown by the installers
# --------------------------------------------------------------------------------------------------------------

def terms_text(info: dict) -> str:
    """The licence page: a short notice, then the GPL in full."""
    full_name = f"{info['name']} {info.get('edition', '')}".strip()
    notice = f"""{full_name} {info['version']}
Copyright (C) 2026 whispr.dev

Please read these terms before installing.

{info['name']} is free software. You may run, study, share and change it under the
GNU General Public License, version 3 only (GPL-3.0-only). The licence is printed
in full below, and installed beside the program as LICENSE.

{info['name']} comes with NO WARRANTY, to the extent permitted by law. You use it at
your own risk, and you are responsible for using it lawfully, including any data
sources you connect it to and their terms of use.

The GPL covers the program. It gives no right to use the {info['name']} name or logo,
which stay with whispr.dev.

Accepting confirms that you have read this notice. It takes away none of the
rights the GPL gives you.

More: {info['url']}

------------------------------------------------------------------------------

"""
    licence = (ROOT / 'LICENSE').read_text(encoding='utf-8')
    return notice + licence


def after_install_windows(info: dict, platform: str) -> str:
    exe = program_name(info, platform)
    return f"""{info['name']} {info['version']} is installed in the folder you chose.

It is a command-line program. To use it, open PowerShell in that folder (in File
Explorer, click the address bar, type powershell and press Enter), then run:

    .\\{exe} --help

To run it by name from any folder, add the folder to your PATH yourself:
Settings > System > About > Advanced system settings > Environment Variables,
select Path under "User variables", click Edit, then New, and paste the folder.

The installer changed nothing else: no registry entries, no PATH change and no
uninstaller. To remove {info['name']}, delete its folder.

Documentation: README.md in the same folder, and {info['url']}
"""


def install_folder(info: dict) -> str:
    """The suggested install folder's name: product.json's "folder", or the program's name."""
    return info.get('folder') or info['name']


def install_script(info: dict, system: str) -> str:
    template = (HERE / 'install.sh').read_text(encoding='utf-8')
    for key, value in (('NAME', info['name']), ('BINARY', info['binary']), ('VERSION', info['version']),
                       ('SYSTEM', system), ('URL', info['url']), ('FOLDER', install_folder(info))):
        # Every value sits inside single quotes in install.sh, so a ' becomes '\'' (close, quoted ', reopen).
        template = template.replace(f'@{key}@', value.replace("'", "'\\''"))
    return template


# --------------------------------------------------------------------------------------------------------------
# archives
# --------------------------------------------------------------------------------------------------------------

def staged_members(info: dict, platform: str, program: Path) -> list:
    """(name inside the folder, source file, mode), the program first."""
    members = [(program_name(info, platform), program, 0o755)]
    for name in info['docs']:
        # The repository's own copy first: in a repository whose crate sits in a subfolder, the top-level
        # README and CHANGELOG are the ones written for users.
        source = ROOT / name if (ROOT / name).is_file() else info['crate'] / name
        if not source.is_file():
            raise Fail(f'{name} is in neither {ROOT} nor {info["crate"]}')
        members.append((Path(name).name, source, 0o644))
    return members


def write_zip(path: Path, folder: str, members: list) -> None:
    with zipfile.ZipFile(path, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, source, mode in members:
            entry = zipfile.ZipInfo(f'{folder}/{name}', date_time=FIXED_DATE)
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = (0o100000 | mode) << 16
            archive.writestr(entry, source.read_bytes(), compresslevel=9)


def write_tar_gz(path: Path, folder: str, members: list) -> None:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w', format=tarfile.PAX_FORMAT) as archive:
        directory = tarfile.TarInfo(folder)
        directory.type, directory.mode, directory.mtime = tarfile.DIRTYPE, 0o755, FIXED_EPOCH
        archive.addfile(directory)
        for name, source, mode in members:
            data = source.read_bytes()
            entry = tarfile.TarInfo(f'{folder}/{name}')
            entry.size, entry.mode, entry.mtime = len(data), mode, FIXED_EPOCH
            archive.addfile(entry, io.BytesIO(data))
    with path.open('wb') as raw, gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0,
                                               compresslevel=9) as packed:
        packed.write(buffer.getvalue())


def fresh(folder: Path) -> Path:
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    return folder


def tool(option: str, variable: str, what: str) -> Path:
    value = option or os.environ.get(variable, '')
    if not value or not Path(value).is_file():
        raise Fail(f'{what} not found: pass its path, or set {variable}')
    return Path(value)


def pack(info: dict, platform: str, args: argparse.Namespace) -> list:
    program = DIST / 'bin' / program_name(info, platform)
    if not program.is_file():
        raise Fail(f'no program at {program}; run the build step first')
    name = stem(info, platform)
    out = DIST / 'out'
    out.mkdir(parents=True, exist_ok=True)
    members = staged_members(info, platform, program)
    made = []
    work = fresh(DIST / 'installer' / platform)
    if platform.startswith('windows'):
        archive = out / f'{name}.zip'
        write_zip(archive, name, members)
        made.append(archive)
        files = fresh(work / 'files')
        for member, source, _ in members:
            shutil.copy2(source, files / member)
        (work / 'TERMS.txt').write_text(terms_text(info), encoding='utf-8-sig', newline='\r\n')
        (work / 'AFTER-INSTALL.txt').write_text(after_install_windows(info, platform), encoding='utf-8-sig',
                                                newline='\r\n')
        if not args.no_installer:
            iscc = tool(args.iscc, 'ISCC', "Inno Setup's ISCC.exe")
            run([iscc, '/Qp',
                 f"/DAppName={info['name']}", f"/DAppVersion={info['version']}",
                 f"/DAppPublisherURL={info['url']}", f'/DAppFolder={install_folder(info)}',
                 f'/DStageDir={files}',
                 f"/DTermsFile={work / 'TERMS.txt'}", f"/DAfterFile={work / 'AFTER-INSTALL.txt'}",
                 f'/DOutputDir={out}', f'/DOutputBase={name}-setup',
                 HERE / 'installer.iss'])
            made.append(out / f'{name}-setup.exe')
    else:
        system = 'macos' if platform.startswith('macos') else 'linux'
        archive = out / f'{name}.tar.gz'
        write_tar_gz(archive, name, members)
        made.append(archive)
        files = fresh(work / 'payload' / 'files')
        for member, source, mode in members:
            shutil.copy2(source, files / member)
            (files / member).chmod(mode)
        script = work / 'payload' / 'install.sh'
        script.write_text(install_script(info, system), encoding='utf-8', newline='\n')
        script.chmod(0o755)
        terms = work / 'TERMS.txt'
        terms.write_text(terms_text(info), encoding='utf-8', newline='\n')
        if not args.no_installer:
            makeself = tool(args.makeself, 'MAKESELF', 'makeself.sh')
            installer = out / f'{name}.run'
            label = f"{info['name']} {info['version']} installer"
            run(['sh', makeself, '--license', terms, '--sha256', '--nox11', '--tar-quietly', '--noprogress',
                 '--keep-umask',
                 '--packaging-date', 'Thu Jan  1 00:00:00 UTC 1980',
                 work / 'payload', installer, label, './install.sh'])
            installer.chmod(0o755)
            made.append(installer)
    for path in made:
        if not path.is_file():
            raise Fail(f'{path.name} was not made')
        print(f'Packed {path.relative_to(ROOT)} ({path.stat().st_size:,} bytes)')
    return made


# --------------------------------------------------------------------------------------------------------------
# release files
# --------------------------------------------------------------------------------------------------------------

def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for block in iter(lambda: handle.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def release_notes(info: dict, names: set) -> str:
    v, b = info['version'], info['binary']

    def link(platform: str, suffix: str) -> str:
        name = f'{b}-{v}-{platform}{suffix}'
        return f'`{name}`' if name in names else 'not built'

    rows = [
        ('Windows 10 or 11, 64-bit', link('windows-x86_64', '-setup.exe'), link('windows-x86_64', '.zip')),
        ('Linux, x86_64', link('linux-x86_64', '.run'), link('linux-x86_64', '.tar.gz')),
        ('Linux, ARM64', link('linux-aarch64', '.run'), link('linux-aarch64', '.tar.gz')),
        ('macOS, Apple silicon and Intel', link('macos-universal', '.run'),
         link('macos-universal', '.tar.gz')),
    ]
    table = '\n'.join(f'| {system} | {installer} | {archive} |' for system, installer, archive in rows)
    # Release pages show each line break, so every paragraph is one line.
    return f"""{info['name']} {info.get('edition', '')} {v}, free software under GPL-3.0-only, from whispr.dev.

## Downloads

| System | Installer | Plain archive |
| --- | --- | --- |
{table}

Each installer shows the licence and asks you to accept it, then asks which folder to install into. It changes nothing else: no registry entries, no PATH changes and no uninstaller. To remove {info['name']}, delete its folder. The plain archives hold the same files, for unpacking yourself.

Each download is also attached without the version in its name (such as `{b}-linux-x86_64.run`). Those copies are identical, and `releases/latest/download/` links to them always give the newest release.

`SHA256SUMS.txt` lists every file's SHA-256 checksum. To check a download on Windows, run `(Get-FileHash .\\FILE -Algorithm SHA256).Hash` in PowerShell; on Linux, `sha256sum FILE`; on macOS, `shasum -a 256 FILE`. The result should match that file's line.

## First run

**Windows.** The installer isn't code-signed yet, so Windows may say "Windows protected your PC". That's expected: click **More info**, then **Run anyway**.

**macOS** (11 or newer on Apple silicon, 10.13 or newer on Intel). Open Terminal and run `sh ~/Downloads/{b}-{v}-macos-universal.run`. Running it with `sh` avoids macOS's warning about programs from the internet that aren't notarised yet.

**Linux.** Open a terminal and run `sh ~/Downloads/{b}-{v}-linux-x86_64.run`, or the ARM64 file.

{info['name']} is a command-line program: run `{b} --help` in the folder you chose. The changes in this release are in `CHANGELOG.md`. More at {info['download_page']}.
"""


def latest_name(info: dict, name: str) -> str:
    """The version-less name of a download: skeletrace-1.0.2-linux-x86_64.run -> skeletrace-linux-x86_64.run."""
    prefix = f"{info['binary']}-{info['version']}-"
    if not name.startswith(prefix):
        raise Fail(f'{name} does not start with {prefix}')
    return info['binary'] + '-' + name[len(prefix):]


def latest_names(info: dict) -> set:
    """Every version-less name a release can carry, so a rerun doesn't copy a copy."""
    suffixes = ('-setup.exe', '.zip', '.run', '.tar.gz')
    return {f"{info['binary']}-{platform}{suffix}" for platform in PLATFORMS for suffix in suffixes}


def latest_copies(info: dict, versioned: list) -> list:
    """Copy each download to its version-less name.

    GitHub serves releases/latest/download/NAME from the newest release, so a web page can link these names
    once and always hand out the newest version.
    """
    copies = []
    for path in versioned:
        copy = path.with_name(latest_name(info, path.name))
        shutil.copyfile(path, copy)
        if os.access(path, os.X_OK):
            copy.chmod(0o755)
        copies.append(copy)
    return copies


def release_files(info: dict, tag: str) -> None:
    if tag != f"v{info['version']}":
        raise Fail(f"tag {tag} does not match the version in Cargo.toml ({info['version']}); "
                   f"tag v{info['version']} instead")
    out = DIST / 'out'
    versioned = sorted(path for path in out.iterdir() if path.is_file() and path.name != 'SHA256SUMS.txt'
                       and path.name not in latest_names(info))
    if not versioned:
        raise Fail(f'nothing to release in {out}')
    files = sorted(versioned + latest_copies(info, versioned))
    sums = ''.join(f'{sha256_of(path)}  {path.name}\n' for path in files)
    (out / 'SHA256SUMS.txt').write_text(sums, encoding='ascii', newline='\n')
    (DIST / 'notes.md').write_text(release_notes(info, {path.name for path in files}), encoding='utf-8',
                                    newline='\n')
    print(sums, end='')
    print(f'Wrote {out / "SHA256SUMS.txt"} and {DIST / "notes.md"}')


# --------------------------------------------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('env', help='print NAME=, BINARY=, VERSION= and CRATE_DIR= lines')
    build_parser = commands.add_parser('build', help='build the program for one platform')
    build_parser.add_argument('--platform', required=True, choices=sorted(PLATFORMS))
    build_parser.add_argument('--target', action='append',
                              help='build for this Rust target instead (repeatable), for trying the packing')
    pack_parser = commands.add_parser('pack', help="pack dist/bin's program for one platform")
    pack_parser.add_argument('--platform', required=True, choices=sorted(PLATFORMS))
    pack_parser.add_argument('--iscc', default='', help="Inno Setup's ISCC.exe (Windows)")
    pack_parser.add_argument('--makeself', default='', help='makeself.sh (Linux and macOS)')
    pack_parser.add_argument('--no-installer', action='store_true', help='make only the plain archive')
    notes_parser = commands.add_parser('release-files', help='write SHA256SUMS.txt and the release notes')
    notes_parser.add_argument('--tag', required=True, help='the git tag being released, such as v1.0.1')
    args = parser.parse_args()
    try:
        info = product()
        if args.command == 'env':
            crate_dir = info['crate'].relative_to(ROOT).as_posix() or '.'
            # Plain \n line ends, even on Windows, for the workflow's GITHUB_ENV file.
            text = (f"NAME={info['name']}\nBINARY={info['binary']}\nVERSION={info['version']}\n"
                    f'CRATE_DIR={crate_dir}\n')
            sys.stdout.buffer.write(text.encode('utf-8'))
            sys.stdout.flush()
        elif args.command == 'build':
            build(info, args.platform, args.target or PLATFORMS[args.platform])
        elif args.command == 'pack':
            pack(info, args.platform, args)
        else:
            release_files(info, args.tag)
    except Fail as problem:
        print(f'package.py: {problem}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
