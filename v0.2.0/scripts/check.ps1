# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
function Invoke-Cargo {
    param([Parameter(Mandatory)][string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}
Push-Location (Join-Path $PSScriptRoot '..')
try {
    Invoke-Cargo -CargoArguments @('fmt', '--all', '--check')
    Invoke-Cargo -CargoArguments @('test', '--locked', '--all-targets')
    Invoke-Cargo -CargoArguments @('test', '--locked', '--doc')
    Invoke-Cargo -CargoArguments @('clippy', '--locked', '--all-targets', '--', '-D', 'warnings')
    Invoke-Cargo -CargoArguments @('build', '--locked', '--release')
    # The offline demo at a fixed time, into a fresh folder that git ignores.
    if (Test-Path -LiteralPath 'target\demo') {
        Remove-Item -LiteralPath 'target\demo' -Recurse -Force
    }
    Invoke-Cargo -CargoArguments @('run', '--locked', '--release', '--', 'demo', '--out', 'target\demo', '--at', '2026-10-01T09:00:00Z')
    # Check the export with SkeleTrace itself when its program is on PATH.
    if (Get-Command skeletrace -ErrorAction SilentlyContinue) {
        Invoke-Cargo -CargoArguments @('run', '--locked', '--release', '--', 'verify-skeletrace', 'target\demo\skeletrace\skeletrace-profile.json')
    }
    else {
        Write-Host 'SkeleTrace is not on PATH; skipped verify-skeletrace.'
    }
    Write-Host 'CrowdWatch validation passed.'
}
finally {
    Pop-Location
}
