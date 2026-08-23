; Dropwire NSIS installer hooks.
;
; Nearby-device discovery (mDNS) and peer-to-peer transfers need INBOUND UDP
; for the app's own binary. Windows blocks it by default, and the prompt users
; get on first launch only covers the CURRENT network profile (so discovery
; silently fails the moment they switch networks). Adding explicit rules for
; all profiles at install time fixes that once, here, with user consent.

!macro NSIS_HOOK_POSTINSTALL
  ; Best-effort: if netsh is missing or refused, the app still works — only
  ; same-network discovery degrades (code/QR sharing is unaffected).
  nsExec::ExecToLog `netsh advfirewall firewall delete rule name="Dropwire" program="$InstDir\Dropwire.exe" `
  nsExec::ExecToLog `netsh advfirewall firewall add rule name="Dropwire" dir=in action=allow program="$InstDir\Dropwire.exe" protocol=udp profile=any enable=yes description="Allow Dropwire to discover and receive transfers from nearby devices" `
  nsExec::ExecToLog `netsh advfirewall firewall add rule name="Dropwire" dir=out action=allow program="$InstDir\Dropwire.exe" protocol=udp profile=any enable=yes description="Allow Dropwire to announce itself and send transfers to nearby devices" `
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Clean up the rules we added (by our unique name + program path).
  nsExec::ExecToLog `netsh advfirewall firewall delete rule name="Dropwire" program="$InstDir\Dropwire.exe" `
!macroend
