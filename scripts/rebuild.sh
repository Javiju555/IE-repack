#!/usr/bin/env bash
#
# SPDX-FileCopyrightText: 2026 Javiju
#
# SPDX-License-Identifier: MIT
#
# Rebuilds a translated, loadable Inazuma Eleven GO Galaxy CCI (.3ds) from:
#   - your own decrypted Japanese CIA of the game, plus either:
#     a) the public .xdelta patch (splice-decrypt + forced decode), or
#     b) the two files that actually carry the translation (ie6_a.fa, ie6_b.fa),
#        either as loose files or extracted on the fly from an already-translated CIA
#
# Why this exists: the official xdelta patch is built against one specific
# byte-exact source file that most people don't have, so applying it directly
# fails with a checksum mismatch against any other (perfectly legitimate) dump
# of the same game. Two ways around that:
#   - xdelta mode: the mismatch is mostly CIA-wrapper + encryption related.
#     Decrypting the base in place (same layout, Secure -> None) and decoding
#     with `xdelta3 -n` (skip checksums) reproduces the translated content:
#     NCCH header, IVFC, Level-3, ExeFS and both .fa files verified byte
#     identical to the reference translated CIA (only dump-version-specific
#     voice clips and the manual stay from your own dump). See ../README.md.
#   - fa modes: sidestep CIA bytes altogether by working at the RomFS file
#     level instead of raw CIA bytes — see ../README.md for the full writeup.
#
# Requires: ctrtool, 3dstool, makerom (all packaged on Arch: ctrtool and
# 3dstool are in the official repos / AUR, makerom via AUR as
# projectctr-makerom-bin). Xdelta mode additionally requires: xdelta3,
# python3 and (only if you point it at the release .zip) unzip.

set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  rebuild.sh --base <japanese.cia> --xdelta-patch <patch.xdelta|patch.zip> --out <output.3ds>
  rebuild.sh --base <japanese.cia> --translated-cia <translated.cia> --out <output.3ds>
  rebuild.sh --base <japanese.cia> --fa-dir <dir with ie6_a.fa + ie6_b.fa> --out <output.3ds>

Options:
  --base            Path to your own decrypted Japanese CIA of the game.
  --xdelta-patch    Path to the public translation patch: either the .xdelta
                     itself or the release .zip that contains it. No translated
                     CIA needed — the translated content is recovered from the
                     patch (see README.md for why plain xdelta fails and why
                     this works).
  --translated-cia  Path to an already-translated CIA to pull ie6_a.fa/ie6_b.fa from.
  --fa-dir          Path to a directory that already contains the translated
                     ie6_a.fa and ie6_b.fa (skips needing the whole translated CIA).
  --out             Output .3ds (CCI) path. Defaults to ./output.3ds
  --keep-work       Don't delete the temporary work directory afterwards.
EOF
}

BASE_CIA=""
TRANSLATED_CIA=""
FA_DIR=""
XDELTA_PATCH=""
OUT="./output.3ds"
KEEP_WORK=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base) BASE_CIA="$2"; shift 2 ;;
    --translated-cia) TRANSLATED_CIA="$2"; shift 2 ;;
    --fa-dir) FA_DIR="$2"; shift 2 ;;
    --xdelta-patch) XDELTA_PATCH="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --keep-work) KEEP_WORK=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Argumento desconocido: $1" >&2; usage; exit 1 ;;
  esac
done

if [[ -z "$BASE_CIA" ]]; then
  echo "Falta --base <japanese.cia>" >&2; usage; exit 1
fi
MODE_COUNT=0
[[ -n "$TRANSLATED_CIA" ]] && MODE_COUNT=$((MODE_COUNT + 1))
[[ -n "$FA_DIR" ]] && MODE_COUNT=$((MODE_COUNT + 1))
[[ -n "$XDELTA_PATCH" ]] && MODE_COUNT=$((MODE_COUNT + 1))
if [[ "$MODE_COUNT" -eq 0 ]]; then
  echo "Falta --xdelta-patch, --translated-cia o --fa-dir" >&2; usage; exit 1
fi
if [[ "$MODE_COUNT" -gt 1 ]]; then
  echo "Elige solo un modo: --xdelta-patch, --translated-cia o --fa-dir" >&2; usage; exit 1
fi
for bin in ctrtool 3dstool makerom; do
  command -v "$bin" >/dev/null 2>&1 || { echo "Falta el binario '$bin' en el PATH." >&2; exit 1; }
done
if [[ -n "$XDELTA_PATCH" ]]; then
  for bin in xdelta3 python3; do
    command -v "$bin" >/dev/null 2>&1 || { echo "El modo --xdelta-patch necesita '$bin' en el PATH." >&2; exit 1; }
  done
  if [[ "$XDELTA_PATCH" == *.zip ]] && ! command -v unzip >/dev/null 2>&1; then
    echo "Para apuntar al .zip del parche hace falta 'unzip' en el PATH (o extrae el .xdelta a mano)." >&2; exit 1
  fi
fi

# OJO: no usar el /tmp por defecto sin más — en muchas distros (incluida
# Arch de serie) es tmpfs en RAM con muy poca capacidad, y este script mueve
# varios GB de datos. Creamos el directorio de trabajo junto al destino de
# salida, que se asume en disco real.
OUT_DIR="$(cd "$(dirname "$OUT")" && pwd)"
WORK="$(mktemp -d --tmpdir="$OUT_DIR" ie-repack.XXXXXX)"
cleanup() { [[ "$KEEP_WORK" -eq 1 ]] || rm -rf "$WORK"; }
trap cleanup EXIT

echo "==> Directorio de trabajo: $WORK"

echo "==> Leyendo cabecera del CIA base..."
CTRTOOL_INFO="$WORK/base_info.txt"
ctrtool -v "$BASE_CIA" > "$CTRTOOL_INFO" 2>&1

hex_field() {
  # hex_field "<etiqueta exacta>" <archivo>
  grep -m1 "$1" "$2" | grep -oE '0x[0-9A-Fa-f]+' | head -1
}

FOOTER_SIZE_HEX=$(hex_field "FooterSize:" "$CTRTOOL_INFO")
CONTENT0_SIZE_HEX=$(awk '/0x0000:/{f=1} f && /Size:/{print; exit}' "$CTRTOOL_INFO" | grep -oE '0x[0-9A-Fa-f]+')
CONTENT1_SIZE_HEX=$(awk '/0x0001:/{f=1} f && /Size:/{print; exit}' "$CTRTOOL_INFO" | grep -oE '0x[0-9A-Fa-f]+')

if [[ -z "$FOOTER_SIZE_HEX" || -z "$CONTENT0_SIZE_HEX" || -z "$CONTENT1_SIZE_HEX" ]]; then
  echo "No se pudieron leer los tamaños del CIA (¿tiene un layout distinto? revisa $CTRTOOL_INFO)" >&2
  exit 1
fi

FOOTER_SIZE=$((FOOTER_SIZE_HEX))
CONTENT0_SIZE=$((CONTENT0_SIZE_HEX))
CONTENT1_SIZE=$((CONTENT1_SIZE_HEX))
TOTAL_SIZE=$(stat -c "%s" "$BASE_CIA")

# El contenido queda justo antes del footer al final del archivo, así que
# calculamos desde el final en vez de fiarnos de offsets/alineación al
# principio del fichero (evita tener que adivinar reglas de padding).
CONTENT0_OFFSET=$((TOTAL_SIZE - FOOTER_SIZE - CONTENT0_SIZE - CONTENT1_SIZE))
CONTENT1_OFFSET=$((CONTENT0_OFFSET + CONTENT0_SIZE))

echo "    content0: offset=$CONTENT0_OFFSET size=$CONTENT0_SIZE"
echo "    content1: offset=$CONTENT1_OFFSET size=$CONTENT1_SIZE"

echo "==> Extrayendo content0 (NCCH principal) y content1..."
dd if="$BASE_CIA" of="$WORK/content0.cxi" bs=1M skip="$CONTENT0_OFFSET" count="$CONTENT0_SIZE" \
   iflag=skip_bytes,count_bytes status=none
dd if="$BASE_CIA" of="$WORK/content1.bin" bs=1M skip="$CONTENT1_OFFSET" count="$CONTENT1_SIZE" \
   iflag=skip_bytes,count_bytes status=none

if [[ -n "$XDELTA_PATCH" ]]; then
  # --- Modo xdelta: base JP + parche público, sin CIA traducido ---
  # El parche falla en aplicación normal por el checksum: la causa principal
  # es que el NCCH de la base del usuario sigue cifrado (Secure) mientras el
  # parche se generó contra una base descifrada (None), más diferencias del
  # wrapper CIA (ticket/TMD únicos por dump). El contenido del juego, en
  # cambio, es el mismo layout, así que: descifrar la base in place
  # (conservando offsets/tamaños exactos) + decode forzado (`-n`) reproduce el
  # contenido traducido byte a byte (verificado contra CIA ESP de referencia:
  # cabecera NCCH, IVFC, Level-3 y ambos .fa idénticos).
  XDELTA_FILE="$XDELTA_PATCH"
  if [[ "$XDELTA_PATCH" == *.zip ]]; then
    echo "==> Extrayendo el .xdelta del .zip del parche..."
    mapfile -t ZIP_XDELTA < <(unzip -Z1 "$XDELTA_PATCH" | grep -i '\.xdelta$' || true)
    if [[ "${#ZIP_XDELTA[@]}" -eq 0 ]]; then
      echo "El .zip no contiene ningún .xdelta." >&2; exit 1
    fi
    if [[ "${#ZIP_XDELTA[@]}" -gt 1 ]]; then
      echo "El .zip trae varios .xdelta (${ZIP_XDELTA[*]}): extrae a mano el que corresponda y pásalo directamente." >&2; exit 1
    fi
    unzip -p "$XDELTA_PATCH" "${ZIP_XDELTA[0]}" > "$WORK/patch.xdelta"
    XDELTA_FILE="$WORK/patch.xdelta"
  fi

  echo "==> Comprobando espacio libre (este modo mueve ~20 GB en $OUT_DIR)..."
  AVAIL_KB=$(df --output=avail "$OUT_DIR" | tail -1 | tr -d ' ')
  if [[ "$AVAIL_KB" -lt 20971520 ]]; then
    echo "Poco espacio en $OUT_DIR (${AVAIL_KB} KB libres, hacen falta ~20 GB)." >&2; exit 1
  fi

  echo "==> Desglosando el NCCH para descifrarlo (3dstool descifra al extraer)..."
  3dstool -xvtf cxi "$WORK/content0.cxi" \
    --header "$WORK/ncchheader.bin" --exh "$WORK/exh.bin" --logo "$WORK/logo.bin" \
    --plain "$WORK/plain.bin" --exefs "$WORK/exefs.bin" --romfs "$WORK/romfs.bin" \
    > "$WORK/extract_ncch.log" 2>&1

  echo "==> Descifrando la base in place (mismo layout, Secure -> None)..."
  python3 - "$BASE_CIA" "$WORK" "$CONTENT0_OFFSET" <<'PYEOF'
import os, struct, sys
base_cia, work, c0_off = sys.argv[1], sys.argv[2], int(sys.argv[3])
parts = {n: open(os.path.join(work, n), "rb").read()
         for n in ("ncchheader.bin", "exh.bin", "exefs.bin", "romfs.bin")}
hdr = parts["ncchheader.bin"]
assert len(hdr) == 0x200, "cabecera NCCH inesperada: %d bytes" % len(hdr)
def u32(o): return struct.unpack("<I", hdr[o:o+4])[0]
exefs_off, exefs_sz = u32(0x1A0)*0x200, u32(0x1A4)*0x200
romfs_off, romfs_sz = u32(0x1B0)*0x200, u32(0x1B4)*0x200
assert len(parts["exefs.bin"]) == exefs_sz, "ExeFS: %d != %d" % (len(parts["exefs.bin"]), exefs_sz)
assert len(parts["romfs.bin"]) == romfs_sz, "RomFS: %d != %d" % (len(parts["romfs.bin"]), romfs_sz)
assert len(parts["exh.bin"]) == 0x800, "exheader inesperado: %d" % len(parts["exh.bin"])
# Copia la base y sustituye solo los rangos cifrados + el byte de flags.
# Todo lo demás (wrapper CIA, logo, plain, tamaños, offsets) queda intacto.
import shutil
spliced = os.path.join(work, "spliced.cia")
shutil.copyfile(base_cia, spliced)
with open(spliced, "r+b") as f:
    f.seek(c0_off + 0x18F); f.write(b"\x04")          # flags: Secure -> None
    f.seek(c0_off + 0x200); f.write(parts["exh.bin"]) # exheader ya descifrado
    f.seek(c0_off + exefs_off); f.write(parts["exefs.bin"])
    f.seek(c0_off + romfs_off)
    data = parts["romfs.bin"]; CH = 64*1024*1024
    for i in range(0, len(data), CH):
        f.write(data[i:i+CH])
print("splice ok: exefs @%s %s, romfs @%s %s" % (
    hex(exefs_off), hex(exefs_sz), hex(romfs_off), hex(romfs_sz)))
PYEOF

  echo "==> Aplicando el parche en modo forzado (los checksums fallan por diseño:"
  echo "    wrapper CIA único por dump + contenido traducido nuevo; el contenido"
  echo "    del juego sale correcto igualmente — ver README.md)..."
  xdelta3 -d -n -s "$WORK/spliced.cia" "$XDELTA_FILE" "$WORK/forced.cia"

  echo "==> Verificando el resultado (TitleId + magia IVFC del RomFS)..."
  python3 - "$BASE_CIA" "$WORK/forced.cia" <<'PYEOF'
import os, re, struct, subprocess, sys
def ncch_header(path):
    out = subprocess.run(["ctrtool", "-v", path],
                         capture_output=True, text=True).stdout
    footer = int(re.search(r"FooterSize:\s+(0x[0-9A-Fa-f]+)", out).group(1), 16)
    # Mismos marcadores que usa el script: 0x0000: / 0x0001: de ContentInfo.
    sizes = []
    for marker in ("0x0000:", "0x0001:"):
        seg = out.split(marker, 1)[1]
        sizes.append(int(re.search(r"Size:\s+(0x[0-9A-Fa-f]+)", seg).group(1), 16))
    total = os.path.getsize(path)
    c0_off = total - footer - sizes[0] - sizes[1]
    with open(path, "rb") as f:
        f.seek(c0_off)
        hdr = f.read(0x200)
        title = hdr[0x108:0x110]
        romfs_off = struct.unpack("<I", hdr[0x1B0:0x1B4])[0]*0x200
        f.seek(c0_off + romfs_off)
        magic = f.read(4)
    return title, magic
tb, _ = ncch_header(sys.argv[1])
tf, magic = ncch_header(sys.argv[2])
assert tb == tf, "TitleId distinto entre base y resultado (¿parche de otro juego?): %s vs %s" % (tb.hex(), tf.hex())
assert magic == b"IVFC", "RomFS sin magia IVFC (parche/base incompatibles): %r" % magic
print("ok: TitleId %s, IVFC presente" % tf.hex())
PYEOF

  echo "==> Reempaquetando el CIA (el TMD del forzado trae los hashes del"
  echo "    contenido original y ya no coinciden: makerom los regenera)..."
  FORCED_INFO="$WORK/forced_info.txt"
  ctrtool -v "$WORK/forced.cia" > "$FORCED_INFO" 2>&1
  F_FOOTER_HEX=$(grep -m1 "FooterSize:" "$FORCED_INFO" | grep -oE '0x[0-9A-Fa-f]+' | head -1)
  F_C0_HEX=$(awk '/0x0000:/{f=1} f && /Size:/{print; exit}' "$FORCED_INFO" | grep -oE '0x[0-9A-Fa-f]+')
  F_C1_HEX=$(awk '/0x0001:/{f=1} f && /Size:/{print; exit}' "$FORCED_INFO" | grep -oE '0x[0-9A-Fa-f]+')
  F_FOOTER=$((F_FOOTER_HEX)); F_C0=$((F_C0_HEX)); F_C1=$((F_C1_HEX))
  F_TOTAL=$(stat -c "%s" "$WORK/forced.cia")
  F_C0_OFF=$((F_TOTAL - F_FOOTER - F_C0 - F_C1))
  F_C1_OFF=$((F_C0_OFF + F_C0))
  dd if="$WORK/forced.cia" of="$WORK/content0_new.cxi" bs=1M skip="$F_C0_OFF" count="$F_C0" \
     iflag=skip_bytes,count_bytes status=none
  dd if="$WORK/forced.cia" of="$WORK/content1_new.bin" bs=1M skip="$F_C1_OFF" count="$F_C1" \
     iflag=skip_bytes,count_bytes status=none
  makerom -f cia -o "$WORK/rebuilt.cia" \
    -content "$WORK/content0_new.cxi:0:0x00000000" \
    -content "$WORK/content1_new.bin:1:0x00000001" \
    -ver 0 -ignoresign > "$WORK/makerom_cia.log" 2>&1

  echo "==> Convirtiendo a CCI (.3ds) — formato que Azahar carga directo, sin pasar"
  echo "    por el instalador de CIA (ver README.md: el instalador rechaza CIAs"
  echo "    sin firma real, incluso los legítimos)."
  makerom -ciatocci "$WORK/rebuilt.cia" -o "$OUT" > "$WORK/makerom_cci.log" 2>&1

  echo "==> Listo: $OUT"
  file "$OUT" || true
  exit 0
fi

echo "==> Desglosando el NCCH con 3dstool..."
3dstool -xvtf cxi "$WORK/content0.cxi" \
  --header "$WORK/ncchheader.bin" --exh "$WORK/exh.bin" --logo "$WORK/logo.bin" \
  --plain "$WORK/plain.bin" --exefs "$WORK/exefs.bin" --romfs "$WORK/romfs.bin" \
  > "$WORK/extract_ncch.log" 2>&1

echo "==> Desempaquetando el RomFS..."
mkdir -p "$WORK/romfs_unpacked"
3dstool -xvtf romfs "$WORK/romfs.bin" --romfs-dir "$WORK/romfs_unpacked" > "$WORK/extract_romfs.log" 2>&1

if [[ -n "$FA_DIR" ]]; then
  echo "==> Copiando ie6_a.fa / ie6_b.fa desde $FA_DIR..."
  cp "$FA_DIR/ie6_a.fa" "$WORK/romfs_unpacked/ie6_a.fa"
  cp "$FA_DIR/ie6_b.fa" "$WORK/romfs_unpacked/ie6_b.fa"
else
  echo "==> Extrayendo ie6_a.fa / ie6_b.fa del CIA ya traducido..."
  TR_ROMFS_DIR="$WORK/translated_romfs"
  mkdir -p "$TR_ROMFS_DIR"
  ctrtool -n 0 --romfsdir="$TR_ROMFS_DIR" "$TRANSLATED_CIA" > "$WORK/extract_translated.log" 2>&1
  cp "$TR_ROMFS_DIR/ie6_a.fa" "$WORK/romfs_unpacked/ie6_a.fa"
  cp "$TR_ROMFS_DIR/ie6_b.fa" "$WORK/romfs_unpacked/ie6_b.fa"
fi

echo "==> Reempaquetando el RomFS (con referencia al original)..."
3dstool -cvtf romfs "$WORK/romfs_new.bin" --romfs-dir "$WORK/romfs_unpacked" \
  --romfs "$WORK/romfs.bin" > "$WORK/repack_romfs.log" 2>&1

echo "==> Reempaquetando el NCCH..."
3dstool -cvtf cxi "$WORK/content0_new.cxi" \
  --header "$WORK/ncchheader.bin" --exh "$WORK/exh.bin" --logo "$WORK/logo.bin" \
  --plain "$WORK/plain.bin" --exefs "$WORK/exefs.bin" --romfs "$WORK/romfs_new.bin" \
  --not-encrypt > "$WORK/repack_ncch.log" 2>&1

echo "==> Construyendo el CIA intermedio con makerom..."
makerom -f cia -o "$WORK/rebuilt.cia" \
  -content "$WORK/content0_new.cxi:0:0x00000000" \
  -content "$WORK/content1.bin:1:0x00000001" \
  -ver 0 -ignoresign > "$WORK/makerom_cia.log" 2>&1

echo "==> Convirtiendo a CCI (.3ds) — formato que Azahar carga directo, sin pasar"
echo "    por el instalador de CIA (ver README.md: el instalador rechaza CIAs"
echo "    sin firma real, incluso los legítimos)."
makerom -ciatocci "$WORK/rebuilt.cia" -o "$OUT" > "$WORK/makerom_cci.log" 2>&1

echo "==> Listo: $OUT"
file "$OUT" || true
