' Windowless launcher for scheduled console commands.
'
' Starts the command line passed as arguments with a hidden window, waits for it,
' and exits with the child exit code so Task Scheduler still records runner
' failures. WScript.exe is a windowless host, so no console window is created at
' any point, unlike a hidden powershell.exe action. `conhost.exe --headless` is
' not an option here: it never returns the child exit code, which would turn
' runner failures into task successes.
Option Explicit

Dim shell, command, index, argument, rc, quoteChar
quoteChar = Chr(34)

If WScript.Arguments.Count < 1 Then
    WScript.Quit 87
End If

command = ""
For index = 0 To WScript.Arguments.Count - 1
    argument = WScript.Arguments(index)
    If InStr(argument, " ") > 0 Then
        If Left(argument, 1) <> quoteChar Or Right(argument, 1) <> quoteChar Then
            argument = quoteChar & argument & quoteChar
        End If
    End If
    If index > 0 Then
        command = command & " "
    End If
    command = command & argument
Next

Set shell = CreateObject("WScript.Shell")
rc = shell.Run(command, 0, True)
WScript.Quit rc
