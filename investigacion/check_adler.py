# SPDX-FileCopyrightText: 2026 Javiju
#
# SPDX-License-Identifier: MIT

import re, zlib, sys, os

# Reutilizable: cambia cia_path a cualquier candidato nuevo que consigas y vuelve a correr.
# log_path se regenera con: xdelta3 printhdrs parche_SN.xdelta > parche_SN_windows.log
log_path = os.path.join(os.path.dirname(__file__), "parche_SN_windows.log")
cia_path = "/home/javiju/Descargas/Inazuma Eleven Go Galaxy - Supernova (Japan).cia"

text = open(log_path, encoding="utf-8", errors="replace").read()
blocks = re.split(r"(?=VCDIFF window number:)", text)

windows = []
for b in blocks:
    if "VCDIFF window number:" not in b:
        continue
    num = re.search(r"VCDIFF window number:\s+(\d+)", b)
    adler = re.search(r"VCDIFF adler32 checksum:\s+([0-9A-Fa-f]+)", b)
    off = re.search(r"VCDIFF copy window offset:\s+(\d+)", b)
    length = re.search(r"VCDIFF copy window length:\s+(\d+)", b)
    if num and adler and off and length:
        windows.append((int(num.group(1)), int(off.group(1)), int(length.group(1)), adler.group(1).upper()))

print(f"Total ventanas parseadas: {len(windows)}")

cia_size = 0
with open(cia_path, "rb") as f:
    f.seek(0, 2)
    cia_size = f.tell()
print(f"Tamaño del CIA candidato: {cia_size} bytes ({cia_size/1e9:.2f} GB)")

match_count = 0
mismatch_first = None
results = []
with open(cia_path, "rb") as f:
    for num, off, length, expected in windows:
        if off + length > cia_size:
            results.append((num, off, length, expected, "FUERA_DE_RANGO", False))
            continue
        f.seek(off)
        data = f.read(length)
        got = format(zlib.adler32(data) & 0xffffffff, "08X")
        ok = (got == expected)
        if ok:
            match_count += 1
        elif mismatch_first is None:
            mismatch_first = (num, off, length, expected, got)
        results.append((num, off, length, expected, got, ok))

print(f"Coinciden: {match_count} / {len(windows)}")
if mismatch_first:
    n, o, l, e, g = mismatch_first
    print(f"Primer fallo -> ventana {n}, offset {o}, length {l}: esperado {e}, obtenido {g}")

# print first 8 and last 8 for a quick visual pattern
print("\nPrimeras 8 ventanas:")
for r in results[:8]:
    print(" ", r)
print("\nUltimas 8 ventanas:")
for r in results[-8:]:
    print(" ", r)
