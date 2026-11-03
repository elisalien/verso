# Verso — Spécification et architecture

Verso est un logiciel de VJing et de mapping vidéo libre (GPL-3.0), multiplateforme
(Windows, macOS, Linux), écrit en Rust sur wgpu.

Le workflow s'inspire des logiciels de VJ à grille de clips (couches × colonnes),
familier aux VJ. Verso est une implémentation **clean-room** : la spécification
provient de la documentation publique, des conventions du métier et de l'usage.
Aucun code, asset, format propriétaire ou élément visuel d'un logiciel tiers n'est
repris ou désassemblé.

## 1. Principes

1. **Jouable à chaque phase.** Chaque version livrée doit servir en live, même limitée.
2. **Le rendu ne bloque jamais.** Décodage, I/O et UI ne peuvent pas faire rater une frame.
3. **Un seul modèle de données.** UI, MIDI, OSC et scripts modifient le même état via des commandes.
4. **Standards ouverts d'abord.** HAP, ISF, OSC, MIDI, NDI, Spout, Syphon, Ableton Link.
5. **Portabilité.** Tout code spécifique à un OS est isolé derrière un trait.

## 2. Vocabulaire

| Terme | Définition |
|---|---|
| Composition | Document racine : couches, colonnes, effets globaux, sorties, réglages. |
| Couche (layer) | Piste verticale de la pile de rendu. Joue au plus un clip à la fois. |
| Colonne | Scène : déclencher une colonne lance le clip de cette colonne sur chaque couche. |
| Clip | Cellule de la grille : une source + transform + chaîne d'effets + réglages de lecture. |
| Source | Générateur de pixels : fichier vidéo, image, shader génératif, entrée live. |
| Effet | Traitement de pixels (ISF, WGSL, natif). Placé sur clip, couche, groupe ou composition. |
| Groupe | Ensemble de couches avec son propre blend, opacité et effets. |
| Deck | Jeu de clips alternatif. Changer de deck change les clips, pas les couches. |
| Paramètre | Valeur animable (float, int, bool, couleur, choix) avec plage et modulation. |
| Sortie (output) | Destination du rendu : fenêtre, écran, slices, Spout/Syphon/NDI. |

## 3. Pipeline de rendu

```
Source → effets clip → transform clip
       → blend dans la couche (crossfade de transition)
       → effets couche → blend dans le groupe
       → effets groupe → blend dans la composition
       → effets composition → master (opacité, crossfader A/B)
       → sorties (fenêtre, slices + warping, partage de texture)
```

- Les couches se composent de bas (index 0) en haut.
- Chaque couche rend dans une texture de taille composition, puis se mélange
  dans un accumulateur en ping-pong (deux textures alternées).
- Format interne : `Rgba16Float`, alpha prémultiplié. Le HDR interne évite le
  banding des blends successifs.
- Modes de mélange v0.1 : Alpha, Add, Multiply, Screen, Lighten, Darken, Difference.
  Les modes sont des shaders, donc la liste est extensible.

## 4. Architecture des threads

| Thread | Rôle | Contrainte |
|---|---|---|
| UI | egui, entrées clavier/souris, édition | Envoie des commandes. Ne touche pas au GPU de rendu. |
| Rendu | Applique les commandes, horloge, rendu wgpu, présentation | Cadence fixe (vsync de la sortie principale). |
| Décodage (N) | Un worker par clip vidéo actif | File de frames pré-décodées, budget mémoire. |
| I/O | MIDI, OSC, Link, NDI | Convertit les messages en commandes. |

La communication passe par des canaux (`crossbeam-channel`) :

- `Command` (UI/I/O → rendu) : déclencher un clip, changer un paramètre, ajouter un effet…
- `Snapshot` (rendu → UI) : état publié en lecture (clips actifs, niveaux, timecode).

En v0.1, UI et rendu partagent le thread principal (eframe). Le découpage en
commandes est respecté dès maintenant pour permettre la séparation plus tard.

## 5. Modèle de données (crate `verso-core`)

```
Composition
├── settings (résolution, fps, bpm)
├── columns: Vec<Column>          nom, couleur
├── layers: Vec<Layer>
│   ├── blend, opacity, bypass, solo
│   ├── clips: Vec<Option<Clip>>  une cellule par colonne
│   ├── active: Option<ClipIndex>
│   └── effects: Vec<Effect>
├── groups: Vec<Group>             (v0.2)
├── effects: Vec<Effect>
└── master: opacity, crossfader
```

- Le modèle est sérialisable (`serde`, RON). Le fichier projet est `*.verso`.
- Aucune dépendance GPU dans `verso-core` : il est testable sans carte graphique.
- Couches et colonnes sont repérées par index. Supprimer, déplacer ou dupliquer
  (v0.3.2) renumérote les mappings MIDI et les modulateurs pour qu'ils suivent
  la même couche ou colonne (`IndexChanges`) ; ceux qui visaient un élément
  supprimé disparaissent. Les adresses écrites dans les scripts Lua ne sont pas
  réécrites.
- Annuler/rétablir (v0.3.2) : instantanés de la composition avant chaque édition
  faite dans l'interface, éditions continues (fader, saisie) fusionnées par
  contrôle sur 1 s, 100 pas. Lancements, MIDI, OSC et scripts ne sont jamais
  enregistrés, et annuler ne change pas ce qui joue.

## 6. Médias

| Format | Chemin de décodage | Phase |
|---|---|---|
| Images (PNG, JPEG, BMP, WebP, TGA, TIFF) | crate `image`, thread de chargement | v0.2 ✅ |
| Vidéo : HAP, H.264, H.265, ProRes, GIF… | processus `ffmpeg` → pipe RGBA, `-hwaccel auto` | v0.2 ✅ |
| HAP natif (upload BC1/BC3/BC7 sans conversion) | démuxage MOV + Snappy | v0.4.1 ✅ |
| Shaders génératifs | WGSL natif, ISF transpilé | v0.1 / v0.2 |

**Décision : FFmpeg en processus, pas libav lié.** Chaque clip vidéo actif lance
un `ffmpeg` qui écrit des frames RGBA brutes sur stdout (crate `verso-media`).
Avantages : aucun header ni `.lib` à installer, tous les codecs de FFmpeg
disponibles, crash d'un décodeur isolé du rendu. Coûts : copie CPU des frames
(≈ 250 Mo/s en 1080p30), seek = relance du processus. Le passage à libav
(`ffmpeg-next`) ou au HAP natif se fera si ces coûts limitent un set réel.
Les binaires sont cherchés dans `PATH`, ou via `VERSO_FFMPEG` / `VERSO_FFPROBE`.

Ouverture vidéo (ffprobe + lancement), chargement d'image et poster tournent
sur des threads ; la boucle de rendu n'attend jamais le disque. Pendant
l'ouverture, la couche affiche le poster du clip.

Le HAP est le format recommandé en live : décodage CPU minimal, scrubbing et
lecture inversée sans coût. **HAP natif (v0.4.1)** : quand le GPU a les textures
BC (`TEXTURE_COMPRESSION_BC`, demandé au démarrage), les fichiers `Hap1` (BC1),
`Hap5` (BC3), `HapY` (HAP Q, YCoCg en BC3 converti dans le shader) et `Hap7`
(BC7) sont lus sans FFmpeg : démuxeur QuickTime minimal (`verso-media::mov`,
tailles vérifiées avant allocation), sections HAP simples ou « complex » par
blocs, Snappy (`snap`). Un thread lit/décompresse en avance dans le sens de
lecture ; le seek est immédiat et la vitesse peut être négative. Hap Q Alpha et
les autres variantes repassent par FFmpeg, qui reste aussi utilisé pour les
vignettes. Vérifié au pixel contre le décodage FFmpeg du même fichier.

Seek FFmpeg : relance du processus avec `-ss` (rapide sur les images clés,
plus lent en H.264/H.265 longue GOP). Les codecs inter-frames (H.264/H.265) ont un scrubbing
lent ; l'UI le signalera et proposera une conversion HAP via FFmpeg.

## 7. Effets

Chaînes d'effets sur quatre niveaux : clip, couche, groupe, composition.
Ordre par couche : source → effets clip → transition → effets couche → blend.
Un effet manquant (fichier absent) est sauté, pas bloquant.

1. **WGSL natif** (`.wgsl`) : en-tête de commentaires (`name`, `category`,
   `param`) et une fonction `effect(uv)`. Verso fournit `sample(uv)`,
   `resolution()`, `premultiply`/`unpremultiply`, `verso.time`, `verso.aspect` et
   un `param_<nom>()` par paramètre. Types : `float`, `color`, `bool`, `point`.
   Format de référence, documenté dans `crates/verso-render/src/effects.rs`.
2. **ISF** (`.fs`) : filtres à une passe et une image d'entrée, traduits en
   GLSL 4.50 puis en WGSL par `naga`. Non supportés pour l'instant : multi-passe,
   buffers persistants, entrées audio, plusieurs images.
3. **Scripts** : Lua pour les automations (voir § 8, Scripts). WASM reste une
   option pour plus tard.

Tout shader est validé par `naga` avant d'être envoyé au GPU : un effet cassé
affiche une erreur au chargement au lieu de faire planter le rendu.
Paramètres : 32 valeurs scalaires par effet (8 `vec4`).

11 effets intégrés : Invert, Hue Saturation, Brightness Contrast, Tint, Posterize,
Blur, Pixelate, Mirror, Kaleidoscope, RGB Split, Transform. Le dossier `effects/`
(répertoire courant ou à côté de l'exécutable) est chargé au démarrage ; le
Browser › Effects permet d'en charger d'autres.

FFGL n'est pas visé : il impose OpenGL, incompatible avec un rendu wgpu sans
interop coûteuse.

### Shaders en ligne et générateurs

Browser › Online cherche et importe des shaders (`verso-online`) :

- **ISF** (editor.isf.video, API publique sans compte) : `/api/search?q=&sort=&from=`,
  source inclus dans les résultats.
- **ShaderToy** : API officielle avec la clé gratuite de l'utilisateur
  (shadertoy.com/myapps), seuls les shaders publiés « Public+API ». Passes Common
  + Image ; `iChannel0` devient l'image d'entrée ; buffers, son, clavier et
  autres canaux refusés avec un message clair.
- **Coller du code ShaderToy** à la main, sans clé.

Les imports vont dans `<dossier utilisateur Verso>/shaders` (`%APPDATA%\Verso`
sous Windows), chargé au démarrage. Un shader sans image d'entrée devient une
**source** de clip (`Source::Shader`, onglet Sources, vignette animée) ; un
shader qui lit son entrée devient un **effet**. Crédit auteur et lien conservés
dans l'en-tête du fichier `.glsl` et affichés dans Properties.

Licences : l'API ShaderToy n'expose pas de licence ; par défaut CC BY-NC-SA 3.0
(pas d'usage commercial). Verso lit les déclarations de licence dans les
commentaires (MIT, CC0, domaine public, Apache, BSD, CC BY…, les mentions
non commerciales l'emportent) pour afficher un badge et filtrer « Commercial
use only ». Heuristique : la page du shader fait foi.

Compatibilité mesurée sur 20 shaders ISF réels : 18 compilent ; échecs restants :
multi-passe, vertex shader personnalisé (`varying`).

### Transitions

Durée par couche (0 = coupe). Au changement de clip, le clip sortant continue
de jouer pendant le fondu (générateurs, shaders, et vidéo : son lecteur est
gardé jusqu'à la fin du fondu) ; l'éjection fond vers le vide. Un changement
pendant un fondu repart de l'image mélangée figée. Seul le fondu enchaîné
existe.

### Groupes

Les couches d'un groupe se mélangent entre elles, puis le groupe reçoit ses
effets et se mélange à la composition avec son propre blend, opacité et bypass.
Un groupe est rendu à la position de sa couche la plus basse.

## 8. Contrôle et I/O

| Protocole | Usage | Phase |
|---|---|---|
| MIDI (`midir`) | Contrôleurs, MIDI learn | v0.3 ✅ |
| OSC (`rosc`) | Contrôle réseau, adresses stables par paramètre | v0.3 ✅ |
| Audio (`cpal` + `realfft`) | Niveaux par bande, beats, tempo détecté | v0.3 ✅ |
| Tap tempo, horloge en beats | Synchro des LFO | v0.3 ✅ |
| Scripts Lua (`mlua`, Lua 5.4) | Automations | v0.3 ✅ |
| Spout (Windows) / Syphon (macOS) | Partage de texture local | v0.4 ✅ (à valider sur machine) |
| NDI® | Sortie et entrée vidéo réseau | v0.4 ✅ |
| Ableton Link | Synchro tempo et phase | v0.4 ✅ |
| SMPTE : MTC / LTC | Timecode, cues | v0.4 ✅ |
| DMX / Art-Net / sACN | Lumières, pixel mapping | plus tard |

### Espace d'adresses

OSC, MIDI et modulation partagent les mêmes cibles (`verso_core::control`).
Adresses OSC, indices à partir de 1, valeurs normalisées 0–1 (un message sans
valeur vaut un appui) :

```
/master  /crossfader  /bpm  /clear
/column/N/launch
/layer/N/opacity|bypass|solo|transition|eject
/layer/N/clip/M/launch
/group/N/opacity|bypass
/composition/effect/I/<param>|bypass
/group/N/effect/I/<param>   /layer/N/effect/I/<param>
/layer/N/clip/M/effect/I/<param>
```

Les déclencheurs réagissent au passage au-dessus de 0,5 (le relâchement d'un
bouton ne fait rien). Port OSC par défaut : 7000, écoute activée depuis la
fenêtre Control. Par défaut seuls les programmes de la machine (127.0.0.1)
peuvent envoyer ; l'option « Accept from other computers » ouvre le réseau,
avec une liste d'adresses autorisées. File bornée (4096 messages), paquets
de plus de 64 bundles imbriqués refusés, valeurs non finies ignorées.

### MIDI learn

Mode Learn (bouton ou touche `L`) : cliquer un contrôle (cellule, en-tête de
colonne, fader, B/S/X de couche, crossfader, paramètre d'effet) le choisit, le
prochain message MIDI (CC ou note) lui est associé. Les mappings sont
enregistrés dans la composition.

### Analyse audio

FFT 1024 points, pas de 512 échantillons. Bandes : grave 20–150 Hz, médium
150–2000 Hz, aigu 2–16 kHz, plus le niveau RMS. Gain automatique par bande
(pic décroissant sur ~10 s, plancher à un quart de la bande la plus forte).
Beats : montée de l'énergie grave au-dessus de 1,3× sa moyenne sur 1 s, 300 ms
minimum entre deux beats. Tempo : autocorrélation de l'enveloppe d'attaque sur
8 s, entre 70 et 180 BPM ; option pour suivre le tempo détecté.

### Modulation

Un modulateur déplace un paramètre d'une fraction (−1 à 1) de sa plage :
LFO (sinus, triangle, scie, carré, aléatoire) en beats du tempo, ou bande audio.
Ajout par clic droit sur un fader ou un paramètre d'effet. Les valeurs
modulées ne sont jamais écrites dans la composition : une copie modulée est
rendue à chaque frame.

### Scripts

Lua 5.4 embarqué (`verso-script`), scripts enregistrés dans la composition et
édités dans la fenêtre Scripts. Un script peut définir `frame(t)` (chaque frame :
`t.time`, `t.dt`, `t.beats`, `t.bpm`, `t.audio.*`) et `beat(n)` (à chaque beat).
API : `verso.set(adresse, valeur)`, `verso.trigger(adresse)`, `verso.get(adresse)`,
`verso.layers()`, `verso.columns()`, `verso.log(...)`, sur le même espace
d'adresses que l'OSC. Bac à sable : bibliothèques `math`, `string`, `table`,
`utf8` seulement (pas d'`io`, `os`, `require`, `load`) et budget de 5 millions
d'instructions par appel : une boucle infinie arrête le script, pas le rendu.
`pcall`/`xpcall` relancent l'erreur de budget (elle ne peut pas être rattrapée)
et la mémoire est limitée à 64 Mio par script.
Un script en erreur s'arrête jusqu'à sa prochaine modification.

### Sorties vers d'autres applications

Capture asynchrone de l'image de sortie (`FrameGrabber`, 3 buffers mappables) :
le rendu ne bloque jamais sur la relecture, l'image arrive avec 1 à 2 frames de
latence. Une sortie active seulement déclenche la capture.

NDI® : runtime chargé à l'exécution (`NDI_RUNTIME_DIR_V6`/`V5`, chemins absolus
seulement sous Windows), aucun SDK requis pour compiler. Envoi RGBA sur un
thread dédié, images sautées si l'envoi est en retard. Entrée : `Source::Ndi`,
un récepteur par couche sur son thread, seule l'image la plus récente est
gardée (RGBX rendu opaque, 8192 px max).

Spout (Windows) : protocole Spout 2 implémenté directement (texture D3D11
partagée BGRA, mémoire partagée `SharedTextureInfo` au nom de l'émetteur, liste
`SpoutSenderNames` protégée par `SpoutSenderNames_mutex`, `<nom>_SpoutAccessMutex`
pendant l'écriture), sans DLL Spout. Syphon (macOS) : `Syphon.framework` chargé
à l'exécution, `SyphonMetalServer` alimenté par une texture Metal. Les deux
partent de l'image relue (copie CPU) et sont vérifiés à la compilation pour leur
plateforme mais **pas encore testés sur machine** : à valider avec
Resolume/TouchDesigner/OBS.

### Ableton Link

Bibliothèque officielle via `rusty_link` (feature `link`, par défaut ; CMake et
C++ requis). Le tempo suit la session ; un changement local (tap, fader, MIDI)
la mène. L'horloge en beats suit la phase de la session (quantum 4) : LFO et
`beat(n)` tombent sur le même temps que les autres applications.

### Timecode

MTC (quarter frames et SysEx full frame) depuis les entrées MIDI ; LTC décodé
depuis un canal de l'entrée audio (biphase mark, 23–31 fps, seuil adaptatif).
Le dernier timecode est extrapolé entre deux trames. Cues : action OSC à une
position ; déclenchées quand la lecture avance de moins d'une seconde au-delà
(un saut ne déclenche rien). `t.timecode` dans les scripts.

### Compteur de lancements

Chaque case (couche × colonne) compte ses lancements, quelle que soit la
source (clic, colonne, MIDI, OSC, script). Le compteur appartient à la case,
pas au média : remplacer le clip le conserve. Affiché sur la vignette et sur
le moniteur Preview, remis à zéro par clic droit. Enregistré avec la
composition.

## 9. Sorties et mapping (v0.5 ✅)

Modèle sans GPU dans `verso_core::mapping`, rendu dans `verso_render::mapping`.

- **Configuration de salle séparée de la composition** : `MappingSetup` (RON,
  `*.versomap`), gardée dans le dossier utilisateur (`mapping.versomap`,
  rechargée au démarrage) et exportable. Réparée à l'ouverture (tailles,
  grilles, valeurs non finies).
- **Écrans** (`Screen`) : un par projecteur, résolution propre, fenêtre
  dédiée (viewport egui) dont la position/taille et le plein écran sont
  mémorisés.
- **Slices** : rectangle d'entrée dans la composition → grille de déformation
  sur l'écran. 2×2 = corner pin avec perspective (homographie) ; jusqu'à
  16×16 = surface Catmull-Rom passant par chaque point, maillée à 16
  subdivisions par case.
- **Edge blending** par bord (largeur 0–50 %, gamma) : rampe smoothstep dont
  deux recouvrements somment à 1 en lumière linéaire.
- **Masques** polygonaux (concaves acceptés, triangulation par oreilles),
  inversables, rendus dans une texture R8 à la taille de l'écran (2048 max).
- Testé : corner pin (coins exacts, centre à l'intersection des diagonales),
  passage par les points, aire du maillage, somme des fondus, aire des
  masques concaves, et rendu GPU au pixel (entrée, déformation, fondu, masque).

Limites : les fenêtres de sortie sont dessinées par le thread UI (pas encore
de thread de rendu séparé) ; pas de choix d'écran par nom (déplacer la fenêtre
puis double-clic) ; pas d'annulation des éditions de mapping.

## 10. Feuille de route

| Version | Contenu | Critère de sortie |
|---|---|---|
| **v0.1 Core** | Grille couches × colonnes, sources génératives WGSL et couleurs, blend modes, opacité, master, preview, sortie plein écran, sauvegarde `.verso` | Un set de 30 min jouable au clavier/souris |
| v0.2 Médias ✅ | FFmpeg, HAP, images, effets ISF/WGSL sur clip/couche/groupe/composition, transitions, groupes | Lecture HAP 1080p × 4 couches à 60 fps (à mesurer) |
| v0.3 Contrôle ✅ | MIDI, OSC, learn, audio FFT/BPM, modulation (LFO, audio), Lua | Set piloté entièrement par contrôleur (à valider sur matériel) |
| v0.4.1 Médias ✅ | HAP natif GPU, seek, lecture arrière, sortante vivante en fondu | 4 × HAP 1080p60 (banc fourni) |
| v0.4 Interop ✅ | Spout, Syphon, NDI entrée/sortie, Link, MTC/LTC, banc de mesure | Échange avec TouchDesigner/OBS (à valider sur machine) |
| v0.5 Mapping ✅ | Écrans, slices, corner pin et grille, edge blending, masques, config de salle | Mapping d'une scène à 2 projecteurs (à valider en salle) |

## 11. Risques techniques

| Risque | Mitigation |
|---|---|
| Interop Spout/Syphon avec wgpu (textures DX11/Metal partagées) | Spike dès v0.1 via les hooks HAL de wgpu ; repli sur copie CPU |
| Textures BC (HAP) indisponibles sur un GPU | Feature `TEXTURE_COMPRESSION_BC` vérifiée au démarrage ; repli décompression CPU |
| Couverture ISF du frontend GLSL de `naga` | Liste de compatibilité, réécriture manuelle des shaders populaires |
| Latence FFmpeg et scrubbing | File pré-décodée par clip, recommandation HAP |
| Charge de travail solo | Phases courtes, critère de sortie mesurable, tests sur `verso-core` |

## 12. Structure du dépôt

```
Verso/
├── Cargo.toml              workspace
├── crates/
│   ├── verso-core/         modèle, commandes, horloge (sans GPU)
│   ├── verso-media/        décodage images et vidéo FFmpeg (sans GPU)
│   ├── verso-control/      OSC, MIDI, analyse audio, tap tempo
│   ├── verso-script/       scripts Lua en bac à sable
│   ├── verso-io/           sorties vers d'autres applications (NDI)
│   ├── verso-online/       recherche et import ISF / ShaderToy
│   ├── verso-render/       moteur wgpu : sources, médias, blend, composition
│   │   ├── src/shaders/    WGSL des sources, blends et sortie
│   │   └── tests/          rendu GPU vérifié au pixel
│   └── verso-app/          application eframe/egui (binaire `verso`)
└── docs/                   spécification, décisions
```
