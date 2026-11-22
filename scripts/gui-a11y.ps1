param([Parameter(Mandatory)][string]$Binary)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
if (-not ('FlitGuiInput' -as [type])) {
    Add-Type -ReferencedAssemblies @([Windows.Automation.AutomationElement].Assembly.Location, [Windows.Automation.AutomationProperty].Assembly.Location) -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Windows.Automation;

public static class FlitGuiInput {
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr param, IntPtr data);

    public static readonly StructureChangedEventHandler TreeChanged = delegate {};

    public static void Post(IntPtr hwnd, uint processId, uint message, int param, int data) {
        uint owner;
        if (GetWindowThreadProcessId(hwnd, out owner) == 0 || owner != processId) {
            throw new InvalidOperationException("The test process no longer owns the window.");
        }
        if (!PostMessage(hwnd, message, new IntPtr(param), new IntPtr(data))) {
            throw new Win32Exception(Marshal.GetLastWin32Error());
        }
    }
}
'@
}

function Wait-For([string]$Description, [scriptblock]$Probe) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $lastError = ''
    while ($clock.Elapsed.TotalSeconds -lt 20) {
        if ($process.HasExited) { throw "Flit exited with status $($process.ExitCode) while waiting for $Description" }
        try {
            $result = & $Probe
            if ($result) {
                Write-Host "ok: $Description"
                return $result
            }
        } catch [Windows.Automation.ElementNotAvailableException] {
            $lastError = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Timed out waiting for $Description. $lastError"
}

function Find-Element($Parent, [string]$Name, [Windows.Automation.ControlType[]]$Types) {
    $named = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $Name)
    $candidates = $Parent.FindAll([Windows.Automation.TreeScope]::Descendants, $named)
    foreach ($element in $candidates) {
        if ($element.Current.ControlType -in $Types) { return $element }
    }
}

function Find-Row([string]$Name) {
    $files = Find-Element $window 'Files' @([Windows.Automation.ControlType]::DataGrid, [Windows.Automation.ControlType]::List)
    if ($files) { Find-Element $files $Name @([Windows.Automation.ControlType]::DataItem, [Windows.Automation.ControlType]::ListItem) }
}

function Find-Edit([string]$Name) {
    Find-Element $window $Name @([Windows.Automation.ControlType]::Edit)
}

function Test-Edit([string]$Name, [string]$Value) {
    $edit = Find-Edit $Name
    return $edit -and $edit.Current.HasKeyboardFocus -and $edit.Current.HelpText -ceq $Value
}

function Invoke-Element($Element) {
    $Element.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
}

function Select-Row([string]$Name) {
    $row = Find-Row $Name
    if (-not $row) { throw "Missing file row $Name" }
    $row.GetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern).Select()
}

function Send-Key([int]$Code) {
    [FlitGuiInput]::Post($hwnd, $process.Id, 0x0100, $Code, 1)
    [FlitGuiInput]::Post($hwnd, $process.Id, 0x0101, $Code, 0xc0000001)
}

function Send-Text([string]$Text) {
    foreach ($unit in $Text.ToCharArray()) {
        [FlitGuiInput]::Post($hwnd, $process.Id, 0x0102, [int]$unit, 1)
    }
}

$exe = (Get-Item -LiteralPath $Binary).FullName
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
$testRoot = [IO.Path]::GetFullPath((Join-Path $tempBase ('flit-gui-' + [guid]::NewGuid().ToString('N'))))
if (-not $testRoot.StartsWith($tempBase + 'flit-gui-', [StringComparison]::OrdinalIgnoreCase)) {
    throw "Cannot isolate the fixture under $tempBase"
}
$process = $null
$window = $null
$listening = $false
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    $files = Join-Path $testRoot 'files'
    $folder = Join-Path $files 'project'
    $original = Join-Path $folder 'draft.txt'
    $hidden = Join-Path $folder '.hidden.txt'
    $cancelled = Join-Path $folder 'ignored.txt'
    $renamed = Join-Path $folder 'renamed.txt'
    $contents = "original contents`n"
    $local = Join-Path $testRoot 'local'
    $state = Join-Path $local 'Flit'
    $roaming = Join-Path $testRoot 'roaming'
    $homePath = Join-Path $testRoot 'home'
    $temp = Join-Path $testRoot 'tmp'
    New-Item -ItemType Directory -Path $folder, $state, $roaming, $homePath, $temp | Out-Null
    [IO.File]::WriteAllText($original, $contents)
    [IO.File]::WriteAllText($hidden, '')
    [IO.File]::SetAttributes($hidden, [IO.FileAttributes]::Hidden)
    [IO.File]::WriteAllText((Join-Path $state 'settings.json'), '{"keys":{"palette.show":["Ctrl+Shift+P","F24"]}}')

    $start = [Diagnostics.ProcessStartInfo]::new($exe, ('"{0}"' -f $files))
    $start.UseShellExecute = $false
    $start.WorkingDirectory = $testRoot
    $variables = $start.EnvironmentVariables
    $variables['LOCALAPPDATA'] = $local
    $variables['APPDATA'] = $roaming
    $variables['USERPROFILE'] = $homePath
    $variables['TEMP'] = $temp
    $variables['TMP'] = $temp
    $variables['FLIT_INSTANCE'] = [IO.Path]::GetFileName($testRoot)
    $variables['FLIT_GUI_TEST'] = '1'
    $variables['FLIT_NO_UPDATE'] = '1'
    $variables['FLIT_LOG'] = '1'
    $variables['FLIT_LANG'] = 'en'
    foreach ($key in 'FLIT_HIDDEN', 'FLIT_HIDDEN_CAPTURE', 'FLIT_RESTARTED_FROM') { $variables.Remove($key) }

    $process = [Diagnostics.Process]::Start($start)
    $hwnd = Wait-For 'Flit window' {
        $process.Refresh()
        if ($process.MainWindowHandle -ne [IntPtr]::Zero) { $process.MainWindowHandle }
    }
    $window = [Windows.Automation.AutomationElement]::FromHandle($hwnd)
    [Windows.Automation.Automation]::AddStructureChangedEventHandler($window, [Windows.Automation.TreeScope]::Subtree, [FlitGuiInput]::TreeChanged)
    $listening = $true
    $folderRow = Wait-For 'folder in the file list' { Find-Row 'project' }
    Invoke-Element $folderRow

    Wait-For 'file in the opened folder' { Find-Row 'draft.txt' } | Out-Null
    if (Find-Row '.hidden.txt') { throw 'The hidden file is visible before toggling hidden files' }
    Select-Row 'draft.txt'
    Wait-For 'selected file' {
        $row = Find-Row 'draft.txt'
        $row -and $row.GetCurrentPattern([Windows.Automation.SelectionItemPattern]::Pattern).Current.IsSelected
    } | Out-Null

    $palette = 'Select a command...'
    $commandName = 'Toggle Hidden Files'
    Send-Key 0x87
    Wait-For 'focused palette search' { Test-Edit $palette '' } | Out-Null
    Send-Text $commandName
    $command = Wait-For 'filtered command result' {
        if (Test-Edit $palette $commandName) { Find-Element $window $commandName @([Windows.Automation.ControlType]::ListItem) }
    }
    Invoke-Element $command
    Wait-For 'hidden file visible after executing the command' { (Find-Row '.hidden.txt') -and -not (Find-Edit $palette) } | Out-Null

    Select-Row 'draft.txt'
    Send-Key 0x71
    Wait-For 'focused rename editor' { Test-Edit 'Rename' 'draft.txt' } | Out-Null
    Send-Text 'ignored'
    Wait-For 'edited name before cancellation' { Test-Edit 'Rename' 'ignored.txt' } | Out-Null
    Send-Key 0x1b
    Wait-For 'rename cancelled in the tree' { (Find-Row 'draft.txt') -and -not (Find-Edit 'Rename') } | Out-Null
    if (-not [IO.File]::Exists($original) -or [IO.File]::Exists($cancelled)) { throw 'Cancelling rename modified the fixture files' }

    Send-Key 0x71
    Wait-For 'rename editor reopened' { Test-Edit 'Rename' 'draft.txt' } | Out-Null
    Send-Text 'renamed'
    Wait-For 'edited file name before commit' { Test-Edit 'Rename' 'renamed.txt' } | Out-Null
    Send-Key 0x0d
    Wait-For 'renamed file and closed editor' { (Find-Row 'renamed.txt') -and -not (Find-Row 'draft.txt') -and -not (Find-Edit 'Rename') } | Out-Null
    if ([IO.File]::Exists($original) -or [IO.File]::ReadAllText($renamed) -cne $contents) {
        throw 'Renaming did not preserve the fixture contents under the new name'
    }
} catch {
    Get-ChildItem -LiteralPath (Join-Path $state 'logs') -Filter '*.log' -File -ErrorAction SilentlyContinue |
        Get-Content -Tail 40 | Write-Host
    throw
} finally {
    try {
        if ($listening) { [Windows.Automation.Automation]::RemoveStructureChangedEventHandler($window, [FlitGuiInput]::TreeChanged) }
    } finally {
        if ($process) {
            if (-not $process.HasExited) { $process.Kill() }
            $process.WaitForExit()
            $process.Dispose()
        }
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
