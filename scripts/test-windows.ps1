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

# Keep the native Job regressions mandatory, including on the independent execution host.
# A successful harness exit with a misspelled selector or missing target executes zero tests.
$containedTests = @(
    'windows_contained::contained_caller_job_lifetimes_and_force_stop',
    'windows_contained::contained_nested_job_checkpoint_recovery_has_one_successor',
    'windows_contained::contained_incompatible_job_refuses_before_worker_execution'
)
$portable = @($tests | Where-Object { $_.target.name -eq 'native_v2_cli_portable' })
if ($portable.Count -ne 1) { throw 'Missing Windows portable CLI test executable' }
$inventory = @(& $portable[0].executable --list)
if ($LASTEXITCODE -ne 0) { throw 'Cannot list Windows portable CLI tests' }
foreach ($required in $containedTests) {
    if ($inventory -notcontains "${required}: test") { throw "Missing Windows regression: $required" }
}

# Direct targets retain file-store ancestor guards while UI/profile state is atomically replaced.
$library = @($tests | Where-Object { $_.target.name -eq 'zeroshot_engine' })
if ($library.Count -ne 1) { throw 'Missing Windows native library test executable' }
$stateInventory = @(& $library[0].executable --list)
if ($LASTEXITCODE -ne 0) { throw 'Cannot list Windows native library tests' }
$stateCommit = 'execution::platform::tests::private_state_round_trips_and_replaces_an_existing_file'
if ($stateInventory -notcontains "${stateCommit}: test") { throw "Missing Windows regression: $stateCommit" }

# The observation suite invokes isolated wire-contract controls. Compile their leaf tests here
# as well, before CI starts the bounded native execution host (including the non-UI profile).
$wireTarget = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR 'wire-contract' } else { Join-Path 'target' 'wire-contract' }
cargo test --locked --package zeroshot --lib --bin zeroshot --target-dir (Join-Path $wireTarget 'current') --jobs 1 --no-run
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test --locked --manifest-path crates/openengine-cluster-testkit/tests/released-wire-contract/Cargo.toml --target-dir (Join-Path $wireTarget 'released') --jobs 1 --no-run
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
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
