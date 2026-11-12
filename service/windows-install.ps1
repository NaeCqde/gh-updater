# Windows (タスクスケジューラ): ログオン時に起動し、落ちたら1分後に再起動する。
# サービスではなくログオンタスクにする理由: 更新対象が画面付きのユーザープロセスのため。
# 管理者権限は不要。パスは環境に合わせて変更。
$exe = "$env:LOCALAPPDATA\gh-updater\gh-updater.exe"
$cfg = "$env:LOCALAPPDATA\gh-updater\updater.windows.toml"
$action   = New-ScheduledTaskAction -Execute $exe -Argument "--config `"$cfg`""
$trigger  = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
            -ExecutionTimeLimit ([TimeSpan]::Zero) -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName "gh-updater" -Action $action -Trigger $trigger -Settings $settings -Force
# 窓を出したくない場合は、タスクの「実行する」を powershell -WindowStyle Hidden 経由にするか、
# 実行ファイルを windows サブシステムでビルドする。
