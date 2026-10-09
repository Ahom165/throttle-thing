#!/usr/bin/env python3
"""
Construit un arbre d'include « insensible à la casse » pour compiler les
headers WDK/SDK Windows sur Linux (sensible à la casse, contrairement à NTFS).

Principe :
 1. Indexe tous les fichiers des roots fournis ; chaque root est projeté dans
    un sous-dossier dédié de l'overlay (le premier root gagne en doublon).
 2. Crée les symlinks réels.
 3. Parse chaque #include "X" / <X> ; si la casse diffère du fichier réel,
    crée un symlink nommé EXACTEMENT comme l'include, dans le sous-dossier
    du root qui contient le fichier.
"""
import os
import re
import sys

BASE = "/home/z/my-project/.wdkbuild"
V = "10.0.26100.0"
# (chemin réel, sous-dossier dans l'overlay)
ROOTS = [
    (f"{BASE}/wdk/c/Include/{V}/km/crt", "km/crt"),
    (f"{BASE}/wdk/c/Include/{V}/km", "km"),
    (f"{BASE}/wdk/c/Include/{V}/shared", "shared"),
    (f"{BASE}/wdk/c/Include/{V}/um", "um"),
    (f"{BASE}/sdkcpp/c/Include/{V}/shared", "sdkshared"),
    (f"{BASE}/sdkcpp/c/Include/{V}/um", "sdkum"),
    (f"{BASE}/sdkcpp/c/Include/{V}/ucrt", "sdkucrt"),
]
OVERLAY = f"{BASE}/overlay"

INC_RE = re.compile(r'^\s*#\s*include\s*[<"]([^<>"]+)[">]', re.M)


def norm(p: str) -> str:
    return p.replace("\\", "/").lower()


def main() -> None:
    # 1. Index
    index = []  # (root, subdir, {rel_lower: rel_exact})
    for root, sub in ROOTS:
        files = {}
        for dirpath, _dirnames, filenames in os.walk(root):
            for fn in filenames:
                rel = os.path.relpath(os.path.join(dirpath, fn), root)
                files[norm(rel)] = rel
        index.append((root, sub, files))

    # 2. Symlinks réels (premier root gagne)
    seen = set()
    for root, sub, files in index:
        for rel_l, rel in files.items():
            if rel_l in seen:
                continue
            seen.add(rel_l)
            dst = os.path.join(OVERLAY, sub, rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            if not os.path.lexists(dst):
                os.symlink(os.path.join(root, rel), dst)

    # 3. Symlinks de casse
    created = 0
    for _root, sub, files in index:
        for rel in files.values():
            path = os.path.join(OVERLAY, sub, rel)
            try:
                with open(path, "r", encoding="utf-8", errors="ignore") as f:
                    text = f.read()
            except OSError:
                continue
            for inc in INC_RE.findall(text):
                inc_n = norm(inc)
                if not inc_n or inc_n not in files:
                    continue
                if files[inc_n] == inc:
                    continue  # casse déjà correcte
                dst = os.path.join(OVERLAY, sub, inc)
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                if not os.path.lexists(dst):
                    os.symlink(os.path.join(_root, files[inc_n]), dst)
                    created += 1
    print(f"overlay prêt : {len(seen)} fichiers, {created} symlinks de casse")


if __name__ == "__main__":
    sys.exit(main())
