#!/usr/bin/env python3
"""Vérification rapide d'un PE : machine, subsystem, entry, imports."""
import struct
import sys

SUBSYS = {1: "NATIVE (kernel)", 2: "WINDOWS_GUI", 3: "WINDOWS_CUI"}
MACH = {0x8664: "x64", 0x14C: "x86", 0xAA64: "ARM64"}


def main(path: str) -> None:
    data = open(path, "rb").read()
    pe_off = struct.unpack_from("<I", data, 0x3C)[0]
    assert data[pe_off:pe_off + 4] == b"PE\0\0", "signature PE absente"
    coff = pe_off + 4
    machine, nsec, _ts, _sym, _nsym, opt_size, chars = struct.unpack_from(
        "<HHIIIHH", data, coff)
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    assert magic == 0x20B, "pas un PE32+"
    entry_rva, _code, _data, image_base = struct.unpack_from("<IIII", data, opt + 16)
    subsystem = struct.unpack_from("<H", data, opt + 68)[0]
    ndd = struct.unpack_from("<I", data, opt + 108)[0]
    ddir = opt + 112

    print(f"Machine   : 0x{machine:04X} ({MACH.get(machine, '?')})")
    print(f"Subsystem : {subsystem} ({SUBSYS.get(subsystem, '?')})")
    print(f"Entry RVA : 0x{entry_rva:X}  |  Image base 0x{image_base:X}")
    print(f"Sections  : {nsec}, caractéristiques 0x{chars:04X}")

    # sections -> rva/file offset map
    sec_off = opt + opt_size
    secs = []
    for i in range(nsec):
        name, _vsize, vaddr, rsize, roff = struct.unpack_from(
            "<8sIIII", data, sec_off + 40 * i)
        secs.append((name.rstrip(b"\0").decode(), vaddr, rsize, roff))

    def rva2off(rva):
        for _n, vaddr, rsize, roff in secs:
            if vaddr <= rva < vaddr + rsize:
                return roff + (rva - vaddr)
        return None

    # import directory
    imp_rva, imp_size = struct.unpack_from("<II", data, ddir + 8)
    print("Imports   :")
    if imp_rva == 0:
        print("  (aucun)")
    off = rva2off(imp_rva)
    while off:
        ilt, _ts, _fc, name_rva, _ft = struct.unpack_from("<IIIII", data, off)
        if name_rva == 0:
            break
        noff = rva2off(name_rva)
        dll = data[noff:data.index(b"\0", noff)].decode()
        print(f"  - {dll}")
        off += 20

    # numéro de version du linker (bonus)
    lmajor, lminor = data[opt + 2], data[opt + 3]
    print(f"Linker    : {lmajor}.{lminor}")


if __name__ == "__main__":
    main(sys.argv[1])
