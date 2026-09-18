param([switch]$Ui)
$ErrorActionPreference = 'Stop'
$features = if ($Ui) { @('--features', 'zeroshot/ui') } else { @() }

# Cargo's Windows Job forbids breakaway. Build first, then launch tests outside that Job
# so detached controllers can exercise their actual lifetime and restrictive-Job behavior.
$artifacts = @(cargo test --workspace @features --no-run --message-format=json | ForEach-Object {
    $_ | ConvertFrom-Json
})
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$tests = @($artifacts | Where-Object {
    $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable
})
if ($tests.Count -eq 0) { throw 'Cargo produced no workspace test executables' }

$failed = $false
foreach ($test in $tests) {
    Push-Location (Split-Path -Parent $test.manifest_path)
    try {
        & $test.executable
        if ($LASTEXITCODE -ne 0) { $failed = $true }
    } finally {
        Pop-Location
    }
}

cargo test --workspace @features --doc
if ($failed -or $LASTEXITCODE -ne 0) { exit 1 }
