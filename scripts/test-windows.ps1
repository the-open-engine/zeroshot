param([switch]$Ui, [switch]$BuildOnly)
$ErrorActionPreference = 'Stop'
$features = if ($Ui) { @('--features', 'zeroshot/ui') } else { @() }

# Cargo's Windows Job forbids breakaway. Build first, then launch tests outside that Job
# so detached controllers can exercise their actual lifetime and restrictive-Job behavior.
# One compiler job limits concurrent metadata mappings on hosted Windows.
$artifacts = @(cargo test --workspace @features --jobs 1 --no-run --message-format=json | ForEach-Object {
    $message = $_ | ConvertFrom-Json
    if ($message.reason -eq 'compiler-message' -and $message.message.rendered) {
        Write-Host $message.message.rendered
    }
    $message
})
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$tests = @($artifacts | Where-Object {
    $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable
})
if ($tests.Count -eq 0) { throw 'Cargo produced no workspace test executables' }
if ($BuildOnly) { exit 0 }

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

cargo test --workspace @features --jobs 1 --doc
if ($failed -or $LASTEXITCODE -ne 0) { exit 1 }
