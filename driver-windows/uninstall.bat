@echo off
REM =====================================================================
REM  uninstall.bat — Décharge le filtre et supprime le service.
REM  À exécuter en tant qu'ADMINISTRATEUR, depuis le dossier du driver.
REM =====================================================================

echo Dechargement du filtre...
fltmc unload throttle 2>nul

echo Suppression du service et des fichiers...
rundll32.exe SETUPAPI.DLL,InstallHinfSection DefaultUninstall 132 .\throttle.inf

echo.
echo Termine. (Pour retirer aussi le mode test-signature :
echo   bcdedit /set testsigning off   puis redemarrer.)
pause
