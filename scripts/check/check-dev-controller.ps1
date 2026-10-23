$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'The native development controller requires Windows.' }

$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$compiler = @('Framework64', 'Framework') | ForEach-Object {
    Join-Path $env:WINDIR "Microsoft.NET\$_\v4.0.30319\csc.exe"
} | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if (-not $compiler) { throw 'The .NET Framework C# compiler is unavailable.' }

$directory = Join-Path ([IO.Path]::GetTempPath()) ('relay-dev-controller-check-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $directory | Out-Null
$executable = Join-Path $directory 'dev-control.exe'
$report = Join-Path $directory 'report.json'
$compilerArgs = @(
    '/nologo', '/utf8output', '/warnaserror+', '/target:winexe', ('/out:' + $executable),
    '/reference:System.Windows.Forms.dll', '/reference:System.Drawing.dll', '/reference:System.Web.Extensions.dll',
    (Join-Path $repo 'scripts\build\dev-control.cs'), (Join-Path $repo 'scripts\build\dev-control.job.cs')
)
& $compiler @compilerArgs
if ($LASTEXITCODE -ne 0) { throw 'Native development controller compilation failed.' }

$process = Start-Process -FilePath $executable -WindowStyle Hidden -PassThru -ArgumentList @(
    '--check', '--self-test', '--repo', ('"' + $repo + '"'), '--report', ('"' + $report + '"')
)
try {
    if (-not $process.WaitForExit(30000)) {
        if (-not $process.HasExited) { $process.Kill() }
        throw 'Synthetic development-controller checks timed out.'
    }
    $result = Get-Content -Raw -LiteralPath $report | ConvertFrom-Json
    if ($process.ExitCode -ne 0 -or -not $result.ok -or -not $result.selfTest -or $result.realRelayStarts -ne 0) {
        throw ('Development-controller checks failed: ' + ($result | ConvertTo-Json -Compress))
    }
    $result | ConvertTo-Json -Compress
    Write-Output "Synthetic acceptance report: $report"
} finally {
    $process.Dispose()
}
