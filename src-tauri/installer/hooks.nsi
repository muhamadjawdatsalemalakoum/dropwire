; Dropwire NSIS installer hooks.
;
; Nearby-device discovery (mDNS) and peer-to-peer transfers need INBOUND UDP
; for the app's own binary. Windows blocks it by default, and the prompt users
; get on first launch only covers the CURRENT network profile (so discovery
; silently fails the moment they switch networks). We add explicit rules for
; all profiles here.
;
; Adding a Windows Firewall rule requires administrator rights, but Dropwire
; installs per-user (installMode = currentUser) so the installer itself runs
; UNELEVATED. The earlier version called `netsh advfirewall` directly, which
; silently failed every time (no admin token). We instead write the netsh
; commands to a batch file and run it once through an elevated shell (one UAC
; prompt). Best-effort: if the user declines elevation the app still works,
; only same-network discovery degrades (code/QR sharing is unaffected).

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  Push $1
  StrCpy $1 "$TEMP\dropwire-firewall.bat"
  FileOpen $0 "$1" w
  FileWrite $0 'netsh advfirewall firewall delete rule name="Dropwire" program="$INSTDIR\Dropwire.exe"$\r$\n'
  FileWrite $0 'netsh advfirewall firewall add rule name="Dropwire" dir=in action=allow program="$INSTDIR\Dropwire.exe" protocol=udp profile=any enable=yes description="Dropwire nearby discovery and incoming transfers"$\r$\n'
  FileWrite $0 'netsh advfirewall firewall add rule name="Dropwire" dir=out action=allow program="$INSTDIR\Dropwire.exe" protocol=udp profile=any enable=yes description="Dropwire announce and outgoing transfers"$\r$\n'
  FileClose $0
  ExecShellWait "runas" "$SYSDIR\cmd.exe" '/c "$1"' SW_HIDE
  Delete "$1"
  Pop $1
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Remove the rules we added (by our unique name + program path), elevated.
  Push $0
  Push $1
  StrCpy $1 "$TEMP\dropwire-firewall-del.bat"
  FileOpen $0 "$1" w
  FileWrite $0 'netsh advfirewall firewall delete rule name="Dropwire" program="$INSTDIR\Dropwire.exe"$\r$\n'
  FileClose $0
  ExecShellWait "runas" "$SYSDIR\cmd.exe" '/c "$1"' SW_HIDE
  Delete "$1"
  Pop $1
  Pop $0
!macroend
