!macro NSIS_HOOK_PREINSTALL
  IfFileExists "$INSTDIR\llm-proxy-worker.exe" 0 llm_proxy_preinstall_complete
  StrCpy $0 1
  ExecWait '"$INSTDIR\llm-proxy-worker.exe" --stop' $0
  IntCmp $0 0 llm_proxy_preinstall_complete
  MessageBox MB_OK|MB_ICONSTOP "The background proxy could not be stopped. Close llm_proxy, wait a moment, and retry this installer."
  Abort
  llm_proxy_preinstall_complete:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  IfFileExists "$INSTDIR\llm-proxy.exe" 0 llm_proxy_preuninstall_complete
  StrCpy $0 1
  ExecWait '"$INSTDIR\llm-proxy.exe" --uninstall' $0
  IntCmp $0 0 llm_proxy_preuninstall_complete
  MessageBox MB_OK|MB_ICONSTOP "The background proxy could not be removed. Close llm_proxy, wait a moment, and retry uninstalling."
  Abort
  llm_proxy_preuninstall_complete:
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "llm_proxy"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "llm-proxy"
!macroend
