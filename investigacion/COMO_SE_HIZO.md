# Cómo se hizo (para más frikis)

Investigación y motor: Javiju555 con Muse Spark (asistente de código, vía
Blazer). Los dumps, parches y traducciones son de sus autores; aquí solo
hay ingeniería inversa y herramienta.

## Galaxy Supernova: fontanería de descifrado

El parche es un xdelta monolítico contra un CIA concreto. Como cada dump
tiene ticket/TMD únicos y el contenido va cifrado, ningún otro dump coincide
al byte: la herramienta descifra tu CIA in place (mismo layout), aplica el
parche en forzado (`-n`) y reinyecta los dos `.fa` traducidos. Verificado
byte a byte contra la copia de referencia (NCCH, IVFC, ExeFS, ambos `.fa`).

## IE 1-2-3: cirugía de sistema de ficheros

El parche de Luis es mejor diseño: xdelta por fichero + `manifiesto.json`
con SHAs original/resultado. El camino:

1. Su base no era otra revisión: código y scripts idénticos a un No-Intro
   limpio (verificado con sus propias herramientas: hooks ARM en su sitio,
   19/19 diálogos iguales). Divergía solo en medios reconstruidos.
2. Su `archive.fa` crece ~200 MB: la herramienta localiza cada fichero por
   verificación de contenido (nombre UTF-16 + SHA, con desempate por voto
   de directorio padre), parchea en estricto y reconstruye el RomFS por
   apéndice (sin tocar el resto de entradas), reensambla el CCI (tabla NCSD
   + tarjeta 4 GB, cabecera original preservada) y re-verifica los 200 SHAs
   desde la imagen final. Arranca en español en Azahar.
3. Rarezas documentadas: CRO con cabecera de 0x80, FileData en ROM 0x162140,
   hashes IVFC L1/L2 que no casan con el estándar (Azahar no los exige).

Ver `NOTA_IE123.md` para la bitácora completa, incluidos los callejones
sin salida (modo forzado = quimera, 20 ventanas caídas, rebases que no
hicieron falta).
