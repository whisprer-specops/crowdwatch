#!/bin/sh
# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
#
# The installer's last step, run by the self-extracting archive (makeself) once the licence has been
# accepted and the files unpacked into a temporary folder. It asks for an install folder and copies the files
# there. It changes nothing else: no PATH edits, no shell profile edits, no system folders.
# package.py fills in the @...@ values, each inside single quotes.
#
#   sh NAME.run                         asks for the folder
#   sh NAME.run -- /path/to/folder      installs there without asking
#   sh NAME.run --accept -- /path       also accepts the licence without asking

set -eu

name='@NAME@'
binary='@BINARY@'
version='@VERSION@'
system='@SYSTEM@'
url='@URL@'
folder='@FOLDER@'

if [ "$system" = macos ]; then
    default="$HOME/Applications/$folder"
else
    default="$HOME/$folder"
fi

target="${1:-}"
if [ -z "$target" ]; then
    if [ -t 0 ]; then
        printf '\nInstall %s %s into which folder?\nPress Enter for %s\n> ' "$name" "$version" "$default"
        read -r answer || answer=''
        target="${answer:-$default}"
    else
        target="$default"
    fi
fi

# A leading ~ means the home folder, as in a terminal, and a relative folder is relative to where the
# installer was started (makeself runs this script from its temporary folder, and keeps the first in USER_PWD).
# shellcheck disable=SC2088 # these are the literal characters the user typed
case "$target" in
    '~') target="$HOME" ;;
    '~/'*) target="$HOME/${target#\~/}" ;;
    /*) ;;
    *) target="${USER_PWD:-$PWD}/$target" ;;
esac

if [ -e "$target" ] && [ ! -d "$target" ]; then
    echo "Cannot install: $target exists and is not a folder." >&2
    exit 1
fi
if ! mkdir -p "$target" 2>/dev/null; then
    echo "Cannot create $target. Choose a folder you can write to, such as $default." >&2
    exit 1
fi

for file in files/*; do
    cp -f "$file" "$target/"
done
chmod 755 "$target/$binary"

# shellcheck disable=SC2088 # shown to the user as typed, not expanded
if [ "$system" = macos ]; then
    profile='~/.zprofile'
else
    profile='~/.profile'
fi

cat <<EOF

$name $version is installed in:
    $target

Try it:
    "$target/$binary" --help

To run it by name from any folder, add this line to $profile and open a new terminal:
    export PATH="$target:\$PATH"

Nothing else on your system was changed. To remove $name, delete that folder.
More: $url
EOF
