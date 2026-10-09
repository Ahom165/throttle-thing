@echo off
REM =====================================================================
REM  install.bat — Installe et charge le minifilter « throttle ».
REM  À exécuter en tant qu'ADMINISTRATEUR, depuis le dossier contenant
REM  throttle.inf et throttle.sys (x64\Release après le build WDK).
REM
REM  Prérequis : mode test-signature activé pour un driver non certifié :
REM     bcdedit /set testsigning on        (redémarrage requis)
REM =====================================================================

net stop throttle 2>nul

echo Installation du driver (throttle.inf)...
rundll32.exe SETUPAPI.DLL,InstallHinfSection DefaultInstall 132 .\throttle.inf
if errorlevel 1 (
    echo [ERREUR] Installation echouee. Verifie que :
    echo   - tu executes ce script en tant qu'administrateur,
    echo   - throttle.inf et throttle.sys sont dans ce dossier,
    echo   - le mode test-signature est actif ^(bcdedit /set testsigning on^).
    pause
    exit /b 1
)

echo Chargement du filtre...
fltmc load throttle
if errorlevel 1 (
    echo [ERREUR] fltmc load a echoue ^(code %errorlevel%^).
    pause
    exit /b 1
)

echo.
echo Termine. Instances attachees :
fltmc instances -f throttle
echo.
echo Lance maintenant l'application (throttle-folder.exe) en administrateur.
pause
