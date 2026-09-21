# Nota 2026-09-15: ¿vale la herramienta para Inazuma Eleven 1·2·3?

## Validación con dump real 2026-09-15 (Vimm/No-Intro, .cci descifrado, 2 GB exactos)

SHA-256 del dump: `35f7497051fd84740e019d40d3632d5943bd265d0a8a9bc77c01112b1b2bfd8d`
(frente al `79bf42d3...` de la base del parche: **no coincide**, como se esperaba).

El modo estricto **rechaza correctamente** (`target window checksum
mismatch`, sin quimera). Diagnóstico fino del v1 (256 ventanas):

- **224/256 ventanas coinciden**, 12 sin ventana de copia, **20 fallan**.
- Las 20 se agrupan en 3 zonas: inicio de fichero (wrapper NCSD/NCCH),
  ~1248–1442 MB (cola de `archive.fa` + `ina_main3ogre.cro`, audios `.SAD`,
  `sound.pb`) y cola (~1928–1952 MB, hashes RomFS + padding `0xFF` de
  cartucho limpio).
- ~190 MB de ficheros de juego (CRO, audio, bancos) difieren de verdad:
  esto **no es varianza de linaje** (el linaje mueve cabeceras/padding,
  no el contenido de los ficheros) → el dump Vimm es **otra revisión**
  del juego que la base del parche (o dicha base es una reconstrucción de
  toolchain, como documenta el propio proyecto: "CIA convertida a
  NCSD... NO equivale al cartucho"). Forzar aquí mezclaría datos de dos
  revisiones: prohibido.
- Conclusión operativa: el mecanismo estricto funciona *como debe* (acepta
  lo idéntico, rechaza lo distinto con mensaje claro). Lo que falta no es
  código, es **rebasar el parche sobre el dump canónico** (este Vimm,
  No-Intro, hash de arriba): entonces todo poseedor del mismo dump aplica
  en estricto, y las bases CIA mediante conversión + puerta de
  contenido (`tail -c +513`).
- Test que lo fija: `gui/crates/ie-core/tests/ie123_strict.rs` (ignorado,
  con los hashes v1/v1.1 publicados). Hoy falla en el paso 1 por diseño; pasará cuando
  la base coincida.

Contexto: el proyecto de traducción al español de **Inazuma Eleven 1·2·3:
Endou Mamoru Densetsu** (3DS, `CTR-P-AETJ`) distribuye parche +
herramientas (sin ROMs) y documenta base descifrada con SHA-256
`79bf42d3f22c6d9e7c7234919ee84007e346fbf421d3faf7ab2b688a0fc4cba2`;
"no sirven las CIA ni las ROMs encriptadas".

## Intel del parche (sin necesitar ninguna ROM, todo público)

En `patch/` hay versiones v1, v1.1-update, v10, v22–v27 (1–22 MB). Cabeceras
VCDIFF (`xdelta3 printhdrs`):

- Fuente: `Inazuma Eleven 1-2-3 - Endou Mamoru Densetsu.3ds` (¡un **CCI
  descifrado**, no un CIA!).
- Target total: 2147483648 bytes exactos (2 GB) en v1 y v27.
- Ventanas de 8 MB con ADDs minúsculos (v1: 7845 bytes en ventana 0, 0 en
  ventana 1; v10: 1756; v27: 424 KB y luego 5). Traducción quirúrgica sobre
  contenedor canónico.
- v1.1 es una **cadena**: v1 sobre la ROM y luego `v1.1-update` sobre el
  resultado, con hashes intermedios/finales publicados
  (`73007ea1...` intermedio, `aa5a9f6c...` final).

## Diagnóstico (teoría, pendiente de un dump real para confirmar)

Mismo síntoma, causa prima hermana — con una diferencia de diseño importante:

- Galaxy: el parche compara un CIA entero (wrapper único por dump) → hizo
  falta modo forzado `-n` + splice-decrypt.
- IE123: el parche compara un CCI descifrado canónico (el dump de cartucho
  descifrado no tiene varianza de empaquetado como los CIA de la escena).
  Si la base descifrada del usuario es la misma versión, el xdelta **normal**
  (con checksums) aplica limpio. Y si falla, significa otra versión de
  verdad — forzar produciría una quimera, así que aquí lo correcto es modo
  **estricto** + hash final como guardián (el proyecto ya publica los hashes).

Predicción falsable: un CCI descifrado con SHA-256 `79bf42d3...` acepta sus
parches con `xdelta3 -d` a secas. No pudimos probarla: no hay dump de este
juego en la máquina y no descargamos ROMs.

## Hallazgo propio al implementar: el wrapper NCSD rompe el hash completo

Leyendo el repo del parche, el build parchea `archive.fa` **in-place** sobre una copia
del CCI: el wrapper (firma RSA real del cartucho en `[0..0x100]`, flags de
tarjeta) queda intacto en la referencia. La conversión desde CIA genera
un wrapper válido pero distinto (relleno makerom + `media_size` por tarjeta,
ahora dinámico: 128 MB–8 GB según imagen). Consecuencia:

- Con base **cartucho** descifrada, el hash completo SÍ puede coincidir
  con el publicado (`aa5a9f...`): el wrapper se preserva tal cual.
- Con base **CIA convertida**, el hash completo no puede coincidir nunca
  (firma distinta), aunque el juego sea byte-idéntico. Por eso la puerta
  acepta también un **SHA-256 de contenido** (desde `0x200`, sin cabecera):
  un proyecto lo publica con `tail -c +513 resultado.3ds | sha256sum`.
  Implementado y probado con fixtures sintéticos (mismo contenido con
  wrapper tocado: pasa contenido, falla completo, como debe).

Esto cuadra además con la nota del proyecto en `docs/PROGRESO.md`: "CIA convertida
localmente a NCSD... La nueva base NO equivale al cartucho usado para los
xdelta antiguos". Exacto: equivale en contenido, no en wrapper.

## Auditoría: qué era específico de Galaxy

Casi nada en el motor: `ie-core` (cia/ncch/cci/xdelta/pipeline) ya calculaba
todo dinámicamente y solo comparaba TitleId base-vs-resultado, nunca fijo.
Específico de Galaxy era solo presentación: tags `BGSJ`/`BGBJ` en la GUI,
título de ventana, nombre de salida por defecto y los fixtures del test
dorado (hashes de `ie6_a.fa`/`ie6_b.fa`).

## Decisión: mismo repo (opción 1 del prompt), ya implementado

- `ie-core`: `normalize` (entrada CIA **o** CCI → CCI descifrado canónico),
  `decode_strict` (sin `-n`, con stderr capturado para diagnosticar),
  `sha256_file`, `run_strict` (cadena de parches + hash final + rename
  atómico, sin temporales de contents).
- GUI: selector de modo. Galaxy igual que siempre; modo genérico
  "Parche .3ds (estricto, experimental)": base `.cia`/`.3ds`, lista ordenada
  de parches multi-selección con borrado por fila, SHA esperado opcional.
- Tests: `strict_synthetic` (cadena v1+v1.1 sintética + rechazo de base
  equivocada) corre en CI con xdelta3 de apt; el round-trip con bytes reales
  queda para cuando haya dump (ver abajo). Layout de ambos modos cubierto
  por el test kittest.

## Qué falta para dar IE123 por soportado (bloqueado, nada técnico)

1. Que alguien con el juego mida su base descifrada contra `79bf42d3...`
   (un `sha256sum`, 1 minuto) — si coincide, el modo genérico debería
   funcionar a la primera.
2. Ideal: una copia ya parcheada de referencia para verificación
   byte a byte como se hizo con Galaxy.
3. Alternativa en casa: si aparece dump + parche de **Big Bang**, valida el
   mismo camino genérico sin depender de terceros.

## Segunda pasada 2026-09-15 (con las herramientas del proyecto de traducción, solo lectura)

- `code.bin` (ExeFS, BLZ) del dump descomprime con el `blz.py` del proyecto
  y los tres hooks de `patch_code.py` caen en prólogos ARM de libro: `strcpy`
  @0x14AC5C (`ldrb r2,[r1]; mov r3,r0; cmp r2,#0; beq`), `getc` @0x1B3788
  (`ldr r1,[r0,#0x10]; add; str; ldrb`), `strcmp` @0x184AAC (`and r3,r0,#3;
  and ip,r1,#3; cmp; mov r2,#0). **Código idéntico a la base del parche.**
- El B123 principal (ROM 0x162140, `data_off` 0x93b14) abre con el
  `fa_unpack.py` del proyecto: **15.547 ficheros**, árbol `inazuma1/2/3(_ogre/_blizzard)`,
  `menu`, `font`, con `eve.pkb/pkh` presentes (IE1: 1293 eventos, incluye
  92010100/92010200 del probe del proyecto).
- Evento 10010001 del `eve.pkb` propio (descomprimido con `lz10.py`,
  parseado con `ssd_records.py`): **19/19 diálogos japoneses idénticos**
  al `translation/game1/dialogo.csv` publicado. **Scripts idénticos.**
- Único B123 top-level en toda la ROM; los otros 16 `B123` son sub-archivos
  anidados (p.ej. dentro de `inazuma2/.../*.arc`). Sin CROs sueltos ni rutas
  `cro/` en el árbol (el `work/romfs/cro/ina_main1.cro` del build no existe
  aquí; los `CRO0` visibles son coincidencias en blobs comprimidos como
  `psh.pkb`, o nombres de entrada anidadas).
- Fecha empotrada `(2012/10/26` en `_PARAM_` (code + archive) y rutas de
  build `ina_main1\src\...` como huellas extra.

## Hipótesis revisada: NO es otra revisión, es base reconstruida

- Código ✓ y scripts ✓ idénticos a dump limpio No-Intro ⇒ la divergencia
  (20 ventanas: wrapper, ~190 MB CRO/audio/cola de datos, padding) está
  confinada a **medios y envoltorio**: justo lo que tocan las herramientas
  del proyecto (`fa_repack`, `mods_to_moflex`, `build_ie1_movies`,
  `ie1_media`) + la nota del proyecto de "base reconstruida que no
  equivale al cartucho".
- Sigue valiendo no forzar (mezclaría el rebuild con el dump limpio).
  Conclusión: no es otra revisión sino base reconstruida; el camino es
  rebasar el parche sobre el dump canónico.

## Aclaraciones del proyecto + giro a file-level (2026-09-15)

- El parche = xdelta sobre ROM `.3ds` JP descifrada (CTR-P-AETJ). ExeFS
  intacto; todo en RomFS: `archive.fa` **reconstruido entero** (en un CIA no
  hay trozos sueltos que coincidan), `cro/ina_main1.cro` (mismo tamaño,
  pocos bytes), 70 `.SAD` (voces EU, mismas rutas).
- Explica los 20 fallos: el archive.fa objetivo es un rebuild completo
  (ventanas ADD + COPYs parciales que no casan con dump limpio). El
  xdelta-sobre-CIA es callejón sin salida.
- El proyecto ofrece probar juntos y salir en el mismo paquete con la
  herramienta portable.
- Verificado además: `archive.fa` y `cro/ina_main1.cro` existen como
  ficheros SUELTOS en el RomFS del dump limpio (entradas UTF-16 únicas en
  0x14bdbc / 0x14be6c; FileData base = ROM 0x162140). `archive.fa` =
  [0x162140, 1309227584) 1.247 GB; `ina_main1.cro` @0x4E093940, 2105344 B,
  con `ldrb r0,[r4]` en 0x36CC0 ✓ y 716 ceros en 0x50E14 ✓ (magia inicial
  `99 4E 7F 8E`: cabecera de 0x80 con CRO0 en 0x80, confirmado después).
- **Estrategia nueva (modelo Galaxy, sin xdelta)**: normalizar CIA/CCI →
  CCI descifrado y REEMPLAZAR ficheros RomFS (archive.fa + CRO + 70 SAD)
  + reconstruir RomFS (rehash IVFC, cambia el tamaño) + reempaquetar.
  Hace falta: set parcheado final + manifiesto (rutas + SHA-256) + tamaño
  del archive.fa objetivo.

## Prototipo file-level VERIFICADO 2026-09-15 (scratch, no en repo)

- Con 3dstool+ctrtool+Azahar del sistema: round-trip CXI extract→create
  **byte-idéntico**; FileData base = ROM 0x162140 (archive.fa dataoff 0);
  FileMeta localizado por nombre UTF-16 + tamaño (1660 rutas, 1690
  entradas con dupes); layout secuencial sin gaps relevantes.
- **Demo A** (swap 2 `.SAD` mismo tamaño): rebuild + splice → ctrtool
  GOOD + **arranca en Azahar hasta el título** (captura).
- **Demo B** (archive.fa +1 MB, 1660 entradas reubicadas, tabla NCSD
  actualizada): oráculo 3dstool confirma tamaños/offsets/contenidos,
  ctrtool GOOD, 60 s de ejecución sin errores.
- Pendiente producto: `romfs.rs` nativo (parser + rebuild + splice,
  validado contra este prototipo), modo GUI "reemplazo de ficheros" con
  manifiesto (ruta + SHA-256), y campo card-size si el archive.fa final
  supera los 2 GB. IVFC L1/L2 del dump original no casan con el layout
  estándar (superblock sí verifica); Azahar no lo exige — anotado como
  riesgo HW, a revalidar con el set real.

## Informe GodMode9 (2026-09-16): tamaños + disección IVFC

- El informe confirma el `0x104` (content size) viejo en nuestra salida:
  implementado (igual a p0) + assert en `manifest_synth` + e2e verde + boot.
- Localizado el L2 real (hashes SHA por bloque de 0x1000, verificado por
  bordes): X=[0x14A000, 0x712C2000), L2=[0x712DF000, 0x72101F00) =
  exactamente 0xE22F00 del superblock. Los apéndices (desde 0x72102000)
  no lo pisan: 256 B de margen. El pánico del solape eran fantasmas
  aritméticos.
- Leído el fuente de GodMode9 (`VerifyNcchFile`/`VerifyNcsdFile`,
  `GetRomFsLvOffset`): el verify a fondo NO puede pasar en este juego ni
  en original (master de 1 hash para un L1 de 28 bloques: over-read de
  heap) — coherente con que su dump limpio tampoco pasa. La consola, en
  cambio, arranca sin pasear el árbol (probado por su vía DeltaPatcher).
- Decisión: solo tamaños (NCCH 0x104 + 0x1B4 ya hecho + NCSD). El L3size
  del IVFC se deja aposta como estaba (agrandarlo rompería la coherencia
  de cobertura L2 que hoy pasa) y no se rehashea nada: boot no lo necesita
  y GodMode9 no puede pasar de todas formas.

- Formato: xdelta por fichero + `manifiesto.json` (ruta, sizes y SHA-256
  original/resultado). 200 ficheros: archive.fa, cro/ina_main1.cro,
  inazuma1/data_iz/sound/*.SAD sueltos (no van dentro de archive.fa).
- Gate: **200/200 SHA originales coinciden** con dump limpio (la base del
  parche = el dump a nivel fichero). Decode estricto 200/200 + gate resultado
  200/200. Traducción verificada a nivel bytes (eventos 92010100,
  10010002 en español decodificado).
- Rebuild: 1660 rutas / 1690 entradas localizadas (voto parent↔dir para
  dupes; mapping 200/200 byte-exacto); delta total +197.742.716;
  imagen final 2.330.284.032 (campo tarjeta → 4 GB, 0x800000).
- Verificación: re-extracción 200/200 SHAs, ctrtool GOOD, **arranca en
  Azahar EN ESPAÑOL** (título "La leyenda de Mark Evans", 55 FPS).
- Detalles de formato: rar dice v55, manifiesto v57-interna (unificar);
  evento 10010001 aún en JP (normal, dev); CRO tiene cabecera 0x80 + CRO0
  en 0x80 (misterio resuelto); el formato por-fichero+manifiesto encaja
  con el modo Pack sin cambios.

## Producto nativo VERIFICADO 2026-09-16 (modo Pack en la GUI)

- `ie-core`: `manifest.rs` (parse+validación), `romfs.rs` (localizar por
  contenido, descubrir base, rebuild por apéndice), `run_manifest`
  (normaliza, 200 puertas dobles, reensambla, re-verifica desde la imagen).
- Episodio loader: el nativo con cabecera regenerada dio una vez "formato
  inválido" en Azahar y luego arrancó siempre (4/4 boots incl. el mismo
  fichero). Causa no aislada (RSA y 0x1C8 descartados por bisección);
  blindaje aplicado: se preserva la cabecera NCSD original y solo se
  parchean slots+tarjeta. Si recurre, mirar `azahar_log.txt` (Loader).
- Tests: units (manifiesto, romfs sintético), `manifest_synth` (pack de 1
  fichero end-to-end con shifts, CI con xdelta3), `manifest_v55` (ignorado,
  200/200 reales). GUI: modo Pack + guía + pie + log a fichero siempre
  activo + test de layout a 780 px en los 3 modos.
- Pendiente: probarlo el usuario, push, que el traductor lo pruebe a fondo; fase 2:
  salida cifrada/CIA para flashcart/CFW.
- CIA N-contenidos 2026-09-16: normaliza descifrando cada CXI con ExeFS
  (los CFA sin ExeFS, p. ej. manuales, viajan como en Galaxy: cifrados,
  Azahar arranca igual); slots 0..N-1 por orden (el del manual puede
  diferir del cartucho, cosmético). Validado con CIA cifrado real de
  Galaxy + minipack de 1 fichero (`manifest_galaxy_cia`, ignorado).
  SIGUIENTE: validar con un CIA real de 3 contenidos (no hay ninguno en
  casa: solo dumps CCI) en cuanto aparezca uno.
- CRO hashes 0x80: la herramienta aplica los bytes tal cual (no recalcula,
  igual que el pipeline del proyecto). En Azahar funciona; en HW con Luma los CRO
  parcheados funcionan en la práctica (la escena los usa a diario).
- Ojo versiones: el mensaje del proyecto dice archive v55 ≈1.454.492.944 pero el
  manifiesto v57 declara resultado 1.505.186.464. El manifiesto manda;
  unificar nombres (rar v55 vs manifiesto v57).
- 2026-09-16: tag v0.2.0-beta.1 CI todo verde, pre-release con 3 zips.
  El aviso visto en el hilo era el warning amarillo de Node 20, no un error.
- Llega un CIA de IE123 (1,8 GB) para validar
  slots de 3 contenidos. Pedido además: renombre general del proyecto +
  portable (ya lo es: exe + sidecars, sin instalador).
- 2026-09-16: Pack + CIA con la beta.1 en GUI: COMPLETADO ("Listo",
  salida 3,46 GB que arranca). Valida slots de 3 contenidos de verdad.
  Episodio "no responde" de GNOME en etapa 4/7: el worker terminó bien por
  debajo (stall de UI/sistema bajo carga, zram casi llena); mitigación:
  cerrar el navegador durante el run. Letras de la GUI agrandadas.
- 2026-09-16: prueba en GUI con el CIA (beta.1): OK
  (Etapa 7/7, salida 3,46 GB que ARRANCA en español). Validación CIA
  completa de verdad, no solo humo.

## v0.3.2: rebuild COMPLETO con rehash IVFC (2026-09-16 noche)

- Fotos de Luis: falla el verify a fondo del Contenido0; el banner carga
  (ExeFS intacto). Espejo fiel de `VerifyNcchFile` en `/tmp/mirror_verify.py`:
  pristina OK 463224 bloques; nuestra v0.3.1 FAIL en lvl3 bloque 1
  (los parches in-place de FileMeta invalidan hashes; el bloque 0 pasa).
- Fix: `rebuild_full` (romfs.rs) reempaqueta el nivel de datos (1660
  entradas, pad a 4, orden original) y recalcula lvl2->lvl1->master con
  tamanos coherentes + hash base NCCH. Detalles: nlen en BYTES, entradas
  alineadas a 4, ultimo bloque parcial se hashea con el relleno a ceros
  (igual que lee GM9), `size_romfs_hash` recalculado.
- Verificado: synth con IVFC real minimo + cadena re-verificada en el
  test; e2e v55 con CCI y con la CIA de Luis (mismo RomFS, otro ExeFS):
  espejo OK 511501 bloques + boot Azahar en ambos.
- CIA de Luis vs CCI Vimm: RomFS identico, ExeFS distinto (otra version).

## CIA original del equipo Galaxy (2026-09-16)

- `IEGOGalaxySupernovaDesencriptado.cia` (equipo; ahora en `Juegos/IE-Galaxy-ES/`) vs nuestro CIA: mismo
  tamaño (2949727232), TitleId y producto iguales, p0 (el juego) BYTE
  IDENTICO tras normalizar (mismos hashes ExeFS/RomFS/ExtHeader).
- Diferencias: (1) el suyo va DESCIFRADO (enc=false), el nuestro CIFRADO
  (enc=true): el xdelta del equipo se genero contra bytes descifrados,
  de ahi el fallo historico al aplicarlo sobre nuestra base (hoy el
  splice-decrypt lo salva). (2) Solo difieren bytes del manual (p1),
  irrelevantes para el parche. Misma revision del juego, sin duda.

## Vimm Galaxy .cci vs CIA del equipo (2026-09-16)

- El .cci de Vimm (4 GB, tarjeta completa) normaliza a enc=false,
  TitleId/producto iguales. p0: ExeFS y RomFS IDENTICOS al CIA del
  equipo; ExtHeader DIFIERE (c97de6.. vs 5fa65a..: metadatos/version,
  no codigo ni datos). p7 de update presente en el cart (los CIA lo
  tiran). Conclusion: mismo juego, distinta envoltura + ExtHeader
  tocado por la conversion a CIA. El xdelta del equipo (contra bytes
  de CIA descifrado) no podia aplicar ni sobre este: contenedor NCSD
  vs CIA, particion de update, tamano 4 GB vs 2,9 GB.

## CIA-izer: modo Galaxy universal (2026-09-16 noche)

- `cia::synthesize_like` + `Inputs.cia_template`: con base CCI/.3ds se
  sintetiza un CIA con el layout de la plantilla (cabecera/TMD/offsets)
  y los contenidos del CCI normalizado (emparejados por tamaño; la
  partición de update se ignora). GUI: tarjeta de plantilla solo si la
  base es .3ds/.cci + botón bloqueado hasta tenerla.
- Experimento SN: Vimm CCI + parche_SN.xdelta + plantilla CIA del equipo
  -> SN-from-cci.3ds OK (TitleId, tamaño idéntico a la referencia ESP,
  arranque Azahar, ESPAÑOL visible). Paridad con la referencia: ambos
  fallan el espejo a fondo (forzado sin rehash, esperado en Galaxy).
- Los tres RomFS fuente (CIA equipo, CIA nuestro, CCI Vimm) son el
  mismo byte (ebbded5..); solo cambian envoltura, ExtHeader y manual.

## Crash HW con 0.4.0 + dump2 (2026-09-17 noche)

- Luis reconstruyó con la 0.4.0 publicada (Pack idéntico a 0.3.2) y el
  crash se repite: `crash_dump_00000001.dmp` es el MISMO punto (PC
  0x08085C50, LR 0x08028A27, `svc 0x3C`, CPSR 0x60000010); los dumps
  solo difieren en SP/pila (ruido). Determinista, con código actual:
  la teoría del "binario viejo" queda descartada para el archivo de
  consola (el `_ESP.3ds` local sí era v0.3.0, pero no es ese archivo).
- Bug cosmético real: la UI mostraba v0.3.0 porque
  `ie-gui/Cargo.toml` seguía en 0.3.0 (el título usa
  `CARGO_PKG_VERSION`). Subido a 0.4.1 (próxima release).
- Rebuild local de su caso exacto (su CIA + pack-fresco) con `main`:
  e2e verde, SHA-256 `1a7d95cd...0d2ed3`, `0x104`==slot, IVFC recrecido,
  flags cripto == nativo. Determinista (== full-cia-v55.3ds).
- Base .3ds de Luis (link gamehub) == CCI Vimm bit a bit (SHA
  `35f74970...2bfd8d`); duplicado eliminado, queda el CCI.
- Pendiente (experimento control): CIA desde el .3ds PRÍSTINO con sus
  mismos pasos GM9 -> ¿arranca? Si también cae en el mismo PC, el
  problema es su conversión (ticket vs contenido plano), no nuestro
  `.3ds`. Pedir además opción exacta del menú GM9 + SHA del `.3ds`.

## Extended header: 1 byte heredado (2026-09-17 noche)

- El extended header (NCCH+0x200, 0x800 B) del rebuild (base CIA Luis)
  difiere del CCI Vimm en UN solo byte: offset 0xD (u32 en 0xC: 1->3,
  "BOLT123" + revisión). El pipeline no escribe ahí y normalize solo
  descifra: es heredado de su CIA (otra revisión), no corrupción
  nuestra. El original con 0x03 arranca, luego no es causa.
- Mapa NCCH confirmado empíricamente: 0x1C0 = hash ExeFS (idéntico en
  ambos -> ExeFS intacto), 0x160 = hash extended header, 0x140 =
  zeros + producto. (OJO: una comparación anterior del ExHeader leyó
  mal offset y dio 0 diff; la buena es esta.)
- Agotada la vía de bytes: lo que viaja al CIA difiere del original
  solo en RomFS traducido (necesario), IVFC coherente, 4 campos NCCH
  (RSA rota, cubierta por Luma en teoría) y ese byte heredado.
  Quedan: paso CIA de GM9 (ticket/cripto; lo decide el control del
  prístino) y RSA-en-arranque (no testeable sin HW).

## Arqueología 2026-09-17: verify, RSA, y plan de experimentos

- Fuente GM9 (`gameutil.c:1755,1808`): el "Verify file" normal llama a
  `VerifyGameFile(path, false)` -> **sig_check=FALSE**. El verde de Luis
  NUNCA comprobó la RSA. La firma solo se mira con la opción aparte
  "Verify signatures". Lo que sí verifica: exthdr vs 0x160, ExeFS
  (superblock + por fichero), IVFC a fondo, tamaños/solapes.
- 3dbrew NCCH: flags nuestros `..01030004` -> [4]=CTR, [5]=Executable,
  [7]=0x04=**NoCrypto** + contenido plano = estado cryptofixed
  consistente. La vía cripto queda descartada por especificación.
- `SetNcchKey`: con NoCrypto no se monta clave (se ignora el ticket).
  `BruteForceNcchCrypto` usa la RSA como oráculo de flags (curiosidad).
- `VerifyCiaFile`: estructura CIA + TMD + SHA por content con titlekey
  del ticket. Pedir a Luis que lo pase a SU cia también.
- Plan: E0 SHA de su .3ds vs 1a7d95cd; E1 CIA del prístino con sus
  pasos (control); E2 HxD 1 bit en zona RSA del prístino (0x4010) ->
  aísla RSA-en-arranque; E3 LayeredFS del traducido sobre instalado
  prístino (exonera contenido + vía de distribución alternativa);
  E4 "verify signatures" a nuestro .3ds (documenta RSA);
  E5 CIA-verify a su CIA; E6 escalar a Luma con dumps + versión FIRM.
- PC/LR sin hits públicos; sin binario FIRM no se simboliza.

## Veredicto HW en consola propia (2026-09-19, Old XL 11.17 + GM9 v2.2.3)

- CFW vía MSET9 (trampa: crear ID1 en mount sin utf8 corrompe el nombre
  a 84 chars y la consola lo ignora; recrear con utf8 -> 32 chars OK).
- NAND+esenciales+SD asegurados en servidor, SHA verificado.
- ESP-v57-rebuild.3ds (nuestro, SHA 1a7d95cd): GM9 verify normal VERDE
  en HW; verify-with-signatures FALLA instantáneo (RSA rota, directo).
- H4 (tamaños/hashes) MUERTA por evidencia HW. Queda H3 (RSA en
  arranque) como única hipótesis viva para nuestro rebuild. Pendiente:
  SD 32GB para pruebas de arranque (E2-boot decisivo + nuestro-boot).

## Reproducción propia (2026-09-20, Old XL 11.17 + GM9 v2.2.3)

- ESP-v57-rebuild.3ds (SHA 1a7d95cd, GM9-green) instalado directo en
  consola propia: cae igual (Arm9, svcBreak). Dump propio:
  PC 0x08085C50, LR 0x08028A27, CPSR 0x60000010. TRIPLE MATCH con los
  dos dumps de Luis (solo SP difiere: ruido de pila). H1 muerta para
  nuestro archivo; H3 (RSA en arranque) única viva. Siguiente: E2-boot.
- Incidencias SD: perfil MSET9-residuo bloqueaba installs (.db);
  corrupción FAT por extracciones sin umount (regla: siempre umount).

## Timing + E1 propio (2026-09-20 noche)

- Prístino en nuestra consola: arranca COMPLETO y jugable -> E1 aquí
  también verde (tarjeta+método+consola+installer validados).
- Nuestro rebuild: animación 3DS + negro, pánico donde el japonés pone
  LEVEL-5 (primer acceso RomFS). .code carga bien (vía ExeFS OK);
  la puerta está en montar/leer RomFS, no en abrir el título.
  Refuerza H3 perezosa (RSA en ruta RomFS) y entierra vía-datos.
- Pendiente E2-boot (instalando).

## Layout no-canónico: 2 fixes (2026-09-21)

- GM9 ignora los logical offsets del IVFC (romfs.h: "seems to be
  useless?") y Azahar deriva: ambos verdes con cualquier valor.
  Nintendo (prístino: L1off=0, L2off=align(s1), L3off=align(s1)+
  align(s2)), makerom (romfs_gen.c: lo encadenado con bloque 0x1000)
  y 3dstool-interno coinciden: offsets VIRTUALES concatenados, no
  físicos. Nosotros escribíamos físicos (GB) -> P9 los usa al montar
  RomFS -> pánico en primer acceso. FIX en rebuild_full.
- 3dbrew: "filedata (aligned to 16-bytes)". Prístino: 1660/1660 doffs
  %16==0 (205 gaps de pad). Nuestro empaquetado contiguo: 916/1660
  desalineados. FIX: cursor alineado a 16 + pad ceros (entra en L3).
- 3dstool (dnasdw, compilado local) valida nuestro blob: extrae 1660
  ficheros, los 200 del manifiesto con SHA resultado OK; fórmulas de
  niveles idénticas a las nuestras (con otra meta, otros tamaños).
