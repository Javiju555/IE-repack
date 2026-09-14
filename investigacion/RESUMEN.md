# Resumen: por qué el parche 1.0.5 (SN) no aplica sobre los CIA/3DS disponibles

Investigación hecha el 2026-09-14. Objetivo: aplicar el parche de traducción al
español de **@IEgogalaxyesp** (v1.0.5, versión Supernova) sobre una base del
juego para jugarlo en Azahar en Linux.

**RESUELTO (2026-09-14, más tarde el mismo día):** en vez de seguir
persiguiendo el `Supernovabase.cia` exacto o portar la traducción a mano,
apareció directamente un CIA ya parcheado (`IEGOGalaxySupernovaESP.cia`,
3.005.760.512 bytes) en un servidor de terceros. Verificado: mismo TitleId
(`000400000010bb00`), mismo product code (`CTR-P-BGSJ`), mismo SaveDataSize
(`0x80000`), y — la prueba definitiva — `strings` sobre el archivo encuentra
diálogo real en español ("El nuevo equipo del Raimon", "Partido contra la
Royal Academy"...) donde el original japonés solo tiene ruido binario
coincidiendo con esas letras. Guardado en `~/Juegos/ROMs/3DS/`, listo para
Azahar sin más pasos.

Todo lo de abajo queda como referencia de por qué el camino del xdelta no
funcionó, y las técnicas de análisis usadas, por si sirven para otro caso
parecido.

## Pista real para arreglar esto de cara a la comunidad (2026-09-14)

Con el ESP ya en mano, se comparó el RomFS completo de la base JP (romsfun.com,
la fuente más estándar que hay hoy — mismo archivo que falló el test de
Adler32 arriba) contra el RomFS del ESP, extrayendo ambos con
`ctrtool -n 0 --romfsdir=...`.

**De 3491 archivos, solo 2 cambian: `ie6_a.fa` y `ie6_b.fa`** (archivos de
Level-5, probablemente "fat archive" empaquetando texto/scripts). Todo lo
demás — `fix.xr`, `gds_pack.pck`, miles de `.bcstm`/`.moflex` — tiene **hash
SHA1 idéntico** entre japonés y español.

Esto confirma que el problema del xdelta no es que el juego sea "distinto" —
es que el xdelta trabaja a nivel de CIA completo (byte-offset absoluto), y
cualquier diferencia de empaquetado entre builds rompe esa comparación aunque
el contenido real sea casi todo idéntico. Un patcher robusto no debería
tocar el CIA como blob:

1. Extraer el RomFS de la base del usuario (`ctrtool --romfsdir`)
2. Sustituir/parchear solo `ie6_a.fa` y `ie6_b.fa`
3. Reempaquetar el RomFS regenerando el árbol de hashes IVFC (pendiente
   verificar herramienta — candidato: `3dstool`, en AUR, no probado aún)

Esto funcionaría contra cualquier CIA japonés correcto de Supernova, no solo
el build exacto que tenía el traductor — a diferencia del enfoque actual del
parche en español. El proyecto en inglés (Level-10 Team / Sxnc,
iegogalaxyeng.netlify.app) usa un patcher propio ("InazumaElevenGoPatcher.exe",
10-30 min de proceso) que por la duración probablemente ya hace algo por el
estilo — pendiente de investigar cómo funciona por dentro.

## Pipeline funcionando de principio a fin (2026-09-14)

Probado y confirmado: reconstruir el CIA en vez de parchearlo evita el
problema del checksum por completo. Receta exacta usada (base: el CIA de
romsfun.com que fallaba el xdelta; traducción: `ie6_a.fa`/`ie6_b.fa` del
CIA ya traducido):

```bash
# 1. Offset del content0 dentro del CIA (calculado desde CIA header: total
#    file size - footer size - content size total, ya que el contenido
#    queda justo antes del footer)
dd if=original.cia of=content0.cxi bs=1M skip=<offset> count=<content0_size> \
   iflag=skip_bytes,count_bytes

# 2. Extraer todas las piezas del NCCH con 3dstool
3dstool -xvtf cxi content0.cxi --header ncchheader.bin --exh exh.bin \
  --logo logo.bin --plain plain.bin --exefs exefs.bin --romfs romfs.bin

# 3. Desempaquetar el RomFS, sustituir los .fa traducidos, reempaquetar
#    con referencia al original (minimiza diferencias)
3dstool -xvtf romfs romfs.bin --romfs-dir romfs_unpacked
cp ie6_a.fa ie6_b.fa romfs_unpacked/   # versiones ESP
3dstool -cvtf romfs romfs_esp.bin --romfs-dir romfs_unpacked --romfs romfs.bin

# 4. Reempaquetar el NCCH/CXI con el RomFS nuevo (sin cifrar, igual que el original)
3dstool -cvtf cxi content0_esp.cxi --header ncchheader.bin --exh exh.bin \
  --logo logo.bin --plain plain.bin --exefs exefs.bin --romfs romfs_esp.bin \
  --not-encrypt

# 5. content1 (segunda partición, ~823KB) se copia sin tocar, mismo método dd

# 6. Reempaquetar el CIA final con makerom (-ignoresign necesario porque
#    reutilizamos un exheader ya firmado por otro proceso, no por makerom)
makerom -f cia -o rebuilt.cia \
  -content content0_esp.cxi:0:0x00000000 \
  -content content1.bin:1:0x00000001 \
  -ver 0 -ignoresign
```

Resultado verificado: TitleId y product code correctos, contenido marcado
`Encrypted: NO`, y el texto en español presente y correcto (mismas cadenas
de diálogo que el CIA ya traducido de referencia). Pendiente: confirmar que
arranca y juega bien en Azahar (no solo que la estructura es válida).

**Limitación de esto tal como está:** esta receta necesita tener ya los
`.fa` traducidos en la mano (los sacamos del CIA ya parcheado que
encontramos). No es todavía un "parche" distribuible de tamaño pequeño — es
una prueba de que reconstruir por RomFS en vez de xdelta sobre el CIA entero
sí resuelve el problema de compatibilidad. Para que sea una herramienta de
verdad útil a la comunidad, faltaría automatizar esto en un script y, si se
quiere un parche ligero en vez de necesitar el CIA traducido completo,
entender el formato interno de `ie6_a.fa`/`ie6_b.fa` para diferenciar solo
las entradas de texto en vez de sustituir el archivo entero.

## CONFIRMADO JUGABLE (2026-09-14, cierre)

El CIA reconstruido (`supernova_esp_rebuilt.cia`) **no instala en Azahar
2126.0** — y tampoco lo hace el CIA japonés original sin tocar de ningún
sitio: `Service.AM: Blocked unauthorized encrypted CIA installation`
(`am.cpp:Write`, código `d8a08004`). Probado también forzando la firma del
ticket a cero (igual que el patrón del CIA japonés original) — mismo
resultado. Conclusión: **el instalador de CIA de esta build de Azahar
rechaza cualquier CIA no firmado de verdad de la comunidad, no es un defecto
de nuestro rebuild.**

La salida: convertir el CIA reconstruido a **CCI (imagen de cartucho,
`.3ds`)** con la propia herramienta de conversión de makerom:

```bash
makerom -ciatocci supernova_esp_rebuilt.cia -o supernova_esp.3ds
```

Un `.3ds`/CCI se carga directo con Archivo → Cargar archivo, sin pasar por
ningún instalador ni chequeo de autorización — es justo el flujo que usaría
un cartucho real insertado. **Confirmado arrancando en Azahar: pantalla de
créditos de la traducción, equipo completo, V.1.0.5, correcto.**

Recomendación para cualquiera que retome esto de cara a la comunidad:
**distribuir como `.3ds`/CCI, no como `.cia`** — evita por completo el lío
de instalación no autorizada que tiene actualmente cualquier CIA
fakeseñado en Azahar reciente.

## Lo que se descartó (no es la causa)

- **No es un problema de claves de cifrado.** `ctrtool` (paquete `ctrtool` en
  los repos oficiales de Arch) trae las claves comunes de Nintendo integradas
  y descifra el ticket de cualquiera de los dos archivos sin configuración
  extra.
- **No es un problema del parcheador.** `DeltaPatcherLite.exe` (bundled en el
  zip del parche) es solo una GUI de wxWidgets que embebe un `xdelta.exe` real
  como recurso (`IDR_XDELTAEXE`) y lo invoca — no reimplementa nada. Usar
  `xdelta3` directamente en Linux es equivalente byte a byte.
- **No es el juego equivocado.** Los dos candidatos probados tienen
  `TitleId: 000400000010bb00` y product code `CTR-P-BGSJ`, que es
  efectivamente Galaxy Supernova. Se verificó explícitamente que en ningún
  momento se mezcló con Big Bang (parche `_SN`, archivos `Supernova`,
  consistente en todas las pruebas).
- **No es el nombre del archivo.** La cabecera VCDIFF del parche declara el
  nombre de origen como `Supernovabase.cia`, pero es solo el nombre local que
  tenía el traductor en su máquina al generar el delta (lo graba xdelta al
  crear el parche) — no existe como descarga pública. Verificado contra los
  dos posts del blog oficial (2020-12 "version1.0-T" y 2021-06 "version 10
  publicada"): ambos piden genéricamente "un CIA desencriptado del juego" sin
  enlace, y en los comentarios la gente pregunta dónde conseguirlo (alguien
  sugiere buscarlo en Twitter).

## Lo que sí se confirmó: mismatch real y total

Con `xdelta3 printhdrs parche_SN.xdelta` se sacó la lista completa de
ventanas VCDIFF del parche (359 ventanas, guardadas en
`parche_SN_windows.log`) — cada una con un checksum Adler32 del fragmento de
archivo origen que espera en un offset/longitud concretos, sin necesitar el
archivo origen para leerlo.

Se comparó ese checksum contra el `.cia` candidato
(`Inazuma Eleven Go Galaxy - Supernova (Japan).cia`, descargado ya
"decrypted" a nivel de contenido — TMD marca `Encrypted: NO` en sus dos
bloques) con el script `check_adler.py`:

```
Coinciden: 0 / 358
Primer fallo -> ventana 0, offset 0: esperado C678C967, obtenido 6CE51B7C
```

**Cero de 358 ventanas coinciden, empezando en el byte 0.** Esto descarta que
sea solo una diferencia de cabecera/empaquetado (si fuera eso, las ventanas
que caen dentro del RomFS/ExeFS puro deberían coincidir aunque fallaran las
primeras). El archivo candidato no es byte-compatible con la fuente del
parche en ningún punto — aunque, importante matiz: esto prueba incompatibilidad
byte a byte, no prueba por sí solo que sea "otra revisión del juego"; una
reconstrucción distinta del mismo contenido (otro empaquetado NCCH/CIA)
también daría este resultado. No se pudo distinguir entre ambas causas con
las pruebas hechas.

Mismo resultado (fallo inmediato de checksum) al probar contra el `.3ds`
(dump de cartucho), como era esperable por ser un contenedor distinto (CCI
vs CIA) aunque sea el mismo juego.

## Dato adicional: cuánto del parche es contenido "nuevo"

Sumando `data section length` de las 359 ventanas: **1,37 GB de 3,01 GB
totales (45,5%) es contenido ADD literal**, no copiado del origen. Es mucho
más de lo esperable para solo texto traducido. Hipótesis (no verificada con
certeza): el RomFS de 3DS usa una estructura de hash tipo Merkle-tree (IVFC,
con niveles Level 0/1/2 — visible en el análisis del `.3ds`), así que cambios
de texto dispersos por muchos archivos de diálogo obligarían a recalcular
bloques de hash en cascada por todo el árbol, inflando mucho el tamaño del
diff aunque el cambio real (el texto) sea pequeño. Esto también complica
bastante la alternativa de "extraer solo los cambios reales y aplicarlos a
mano sobre tu propio RomFS" — no es un diff de archivos de texto sueltos, hay
que lidiar con el árbol de integridad.

## Dónde conseguir la fuente correcta (pendiente)

El propio blog no da el enlace en los posts revisados. Pistas para seguir:
- Comentarios del blog mencionan Twitter como fuente para conseguir el CIA
  base — cuenta asociada: `@IEgogalaxyesp`.
- Posts más antiguos (2020) mencionaban enlaces "Descargar Supernova" /
  "Descargar BigBang" que en 2021 dejaron de estar en el procedimiento
  publicado (cambiaron a "parche + CIA que consigas tú").
- Si esa fuente ya no está disponible (comunidad/foro inactivo), la única vía
  con garantías de éxito es encontrar exactamente el build que ellos usaron —
  no un "CIA decrypted" genérico de un sitio de ROMs cualquiera, por muy
  correcto que parezca el título/versión a simple vista.

## Herramientas dejadas listas para retomar esto

- `ctrtool` y `xdelta3`: instalados vía pacman.
- `parche_SN_windows.log`: dump completo de las 359 cabeceras VCDIFF, no hace
  falta volver a extraer el parche para consultarlo.
- `check_adler.py`: cambia `cia_path` a cualquier candidato nuevo y compara al
  instante contra las 358 ventanas sin necesidad de un intento de parcheo
  completo (que tarda mucho más y no da tanta información al fallar).
