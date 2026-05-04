# install-recorder-task.ps1 - register the deskmic-ingest-recorder scheduled task.
# Mirrors the existing deskmic-watchdog and deskmic-index-and-sync tasks.

param(
    [int]$IntervalMinutes = 15,
    [string]$TaskName = "deskmic-ingest-recorder"
)

$exe = "$env:USERPROFILE\.cargo\bin\deskmic.exe"
if (-not (Test-Path $exe)) {
    throw "deskmic.exe not found at $exe - install it first."
}

$action = New-ScheduledTaskAction -Execute $exe -Argument "ingest-recorder"
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) `
    -RepetitionInterval (New-TimeSpan -Minutes $IntervalMinutes)
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME `
    -LogonType Interactive -RunLevel Limited

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
    -Settings $settings -Principal $principal -Force | Out-Null

Write-Output "Registered scheduled task '$TaskName' (every $IntervalMinutes minutes)."
