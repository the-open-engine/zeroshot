param([string]$State)
$ErrorActionPreference = 'Stop'

if ($State) {
    $status = 1
    try {
        $context = Import-Clixml (Join-Path $State 'context.xml')
        foreach ($entry in $context.Environment.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process')
        }
        Set-Location $context.Workspace
        & $context.Shell -NoProfile -NonInteractive -File ./scripts/test-windows.ps1 *> (Join-Path $State 'output.log')
        $status = $LASTEXITCODE
    } catch {
        $_ | Out-File (Join-Path $State 'output.log') -Append
    } finally {
        [IO.File]::WriteAllText((Join-Path $State 'exit-code'), [string]$status)
    }
    exit $status
}

# GitHub's hosted-compute-agent puts its entire runner in a non-breakaway Job.
# WMI starts this CI-only test host independently, with the same caller identity.
$State = Join-Path $env:RUNNER_TEMP ('zeroshot-tests-' + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $State | Out-Null
$environment = @{}
foreach ($name in @('PATH', 'HOME', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'TEMP', 'TMP', 'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN', 'RUNNER_TRACKING_ID')) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if ($null -ne $value) { $environment[$name] = $value }
}
$context = @{
    Shell = (Get-Process -Id $PID).Path
    Workspace = (Get-Location).Path
    Environment = $environment
}
$context | Export-Clixml (Join-Path $State 'context.xml')
$process = $null
try {
    $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ CreateFlags = [uint32]0x01000000 }
    $command = '"' + $context.Shell + '" -NoProfile -NonInteractive -File "' + $PSCommandPath + '" -State "' + $State + '"'
    $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine = $command
        CurrentDirectory = $context.Workspace
        ProcessStartupInformation = $startup
    }
    if ($created.ReturnValue -ne 0) { throw "Windows test host creation failed: $($created.ReturnValue)" }
    $process = Get-Process -Id $created.ProcessId
    if (!$process.WaitForExit(1800000)) { throw 'Windows test host exceeded thirty minutes' }
    $exitCode = Join-Path $State 'exit-code'
    if (!(Test-Path $exitCode)) { throw 'Windows test host exited without a result' }
    $status = [int](Get-Content $exitCode -Raw)
} finally {
    if ($process -and !$process.HasExited) {
        $process.Kill($true)
        if (!$process.WaitForExit(5000)) { throw 'Windows test host cleanup timed out' }
    }
    if ($process) { $process.Dispose() }
    $log = Join-Path $State 'output.log'
    if (Test-Path $log) { Get-Content $log }
    Remove-Item -Recurse -Force $State
}
exit $status
