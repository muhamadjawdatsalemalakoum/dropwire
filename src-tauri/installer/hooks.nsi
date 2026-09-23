; Dropwire NSIS installer hooks.
;
; Nearby-device discovery (mDNS) and peer-to-peer transfers need INBOUND UDP
; for the app's own binary. Windows blocks it by default, and the prompt users
; get on first launch only covers the CURRENT network profile (so discovery
; silently fails the moment they switch networks). We add an explicit inbound
; rule for all profiles here. Outbound traffic is allowed by default, so no
; outbound rule is needed.
;
; Adding a firewall rule requires administrator rights, but Dropwire installs
; per-user (installMode = currentUser), so the installer itself runs
; UNELEVATED. The rule is added by running netsh.exe itself elevated (one UAC
; prompt, which names Microsoft's Network Command Shell), with the whole
; command on its command line. Nothing is written to a file first: a script in
; %TEMP% could be changed by another program while the prompt is up, and would
; then run with administrator rights.
;
; The prompt only appears when it is needed. Listing rules needs no rights, so
; the installer first looks for its rule for this exact program and skips the
; prompt when it is already there (reinstalls and upgrades). The rule has its
; own name because Windows names the rules from its first-run prompt after the
; app ("Dropwire"), and listing all of those would not fit in an NSIS string.
; The uninstaller removes this program's rules only when it finds that rule,
; and leaves them when an installer started it to replace the app, because the
; new version keeps using them. Best-effort throughout: if the user declines,
; the app still works; only same-network discovery degrades (code/QR sharing
; is unaffected).

!ifndef DROPWIRE_FW_RULE
  !define DROPWIRE_FW_RULE "Dropwire nearby"
!endif

; Set RESULT to 1 when the inbound rule named DROPWIRE_FW_RULE exists for
; $INSTDIR\Dropwire.exe, else 0. Needs no administrator rights. RESULT must
; not be one of $R0-$R5, which this uses and restores.
!macro DropwireFirewallRuleExists RESULT
  Push $R0
  Push $R1
  Push $R2
  Push $R3
  Push $R4
  Push $R5
  StrCpy $R4 0
  nsExec::ExecToStack /OEM '"$SYSDIR\netsh.exe" advfirewall firewall show rule name="${DROPWIRE_FW_RULE}" dir=in verbose'
  Pop $R0 ; exit code: 0 when a rule matched, 1 for "No rules match"
  Pop $R1 ; output, cut off at NSIS_MAX_STRLEN
  ${If} $R0 == 0
    ; Look for the program path in the listing. == ignores case, as Windows
    ; does for paths.
    StrCpy $R2 "$INSTDIR\Dropwire.exe"
    StrLen $R3 $R2
    StrCpy $R0 0
    ${Do}
      StrCpy $R5 $R1 $R3 $R0
      ${If} $R5 == ""
        ${ExitDo}
      ${EndIf}
      ${If} $R5 == $R2
        StrCpy $R4 1
        ${ExitDo}
      ${EndIf}
      IntOp $R0 $R0 + 1
    ${Loop}
  ${EndIf}
  StrCpy ${RESULT} $R4
  Pop $R5
  Pop $R4
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  !insertmacro DropwireFirewallRuleExists $0
  ${If} $0 != 1
    ExecShellWait "runas" "$SYSDIR\netsh.exe" 'advfirewall firewall add rule name="${DROPWIRE_FW_RULE}" dir=in action=allow program="$INSTDIR\Dropwire.exe" protocol=udp profile=any enable=yes description="Dropwire nearby discovery and incoming transfers"' SW_HIDE
  ${EndIf}
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Push $0
  ; A normal uninstall copies the uninstaller to %TEMP% and runs it from
  ; there. It runs in place, from the install folder, only when an installer
  ; started it (with _?=) to replace the app; the new version keeps the rule.
  ${If} $EXEDIR != $INSTDIR
    !insertmacro DropwireFirewallRuleExists $0
    ${If} $0 == 1
      ; Every rule for this program: ours, the in/out pair older installers
      ; added, and any that Windows created from its own first-run prompt.
      ExecShellWait "runas" "$SYSDIR\netsh.exe" 'advfirewall firewall delete rule name=all program="$INSTDIR\Dropwire.exe"' SW_HIDE
    ${EndIf}
  ${EndIf}
  Pop $0
!macroend
