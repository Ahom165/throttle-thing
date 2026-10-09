@echo off
REM =====================================================================
REM  uninstall-noinf.bat — Decharge le filtre et supprime le service
REM  installe par install-noinf.bat. En administrateur.
REM =====================================================================

echo Dechargement du filtre...
fltmc unload throttle 2>nul

echo Suppression du service et des cles de registre...
sc delete throttle >nul 2>&1
reg delete HKLM\SYSTEM\CurrentControlSet\Services\throttle /f >nul 2>&1

echo.
echo Termine. (Pour quitter le mode test-signature :
echo   bcdedit /set testsigning off   puis redemarrer.)
pause
