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
- Les identifiants stables (`LayerId`, `ClipId`) arriveront avec l'édition
  (déplacement de couches) ; v0.1 utilise des index.

### Document, annulation et dialogues

Chaque `Command` a un `CommandKind` : `Play` (déclencher, éjecter : ne modifie
pas le document), `Tweak` (faders, paramètres, noms : modifie le document,
trop fréquent pour une annulation pas à pas) ou `Edit` (clips, couches,
colonnes, groupes, chaînes d'effets, mappings : modifie le document et
s'annule). L'app marque la composition « non enregistrée » sur tout `Tweak`
ou `Edit`, demande confirmation avant Nouveau / Ouvrir / Quitter, et garde
50 instantanés pour `Ctrl+Z` / `Ctrl+Shift+Z` (restaurer un instantané
conserve les clips en cours de lecture). `RemoveLayer` et `RemoveColumn`
décalent ou suppriment les modulateurs et mappings MIDI concernés, comme
`RemoveGroup` pour les groupes.

Toutes les deux minutes, une composition modifiée est copiée dans
`nom.verso.bak` à côté de son fichier (ou `Untitled.verso.bak` dans le
dossier utilisateur si elle n'a pas encore de fichier). Enregistrer supprime
la copie. Un échec d'enregistrement automatique n'interrompt jamais le set :
il va seulement dans le journal.

Le journal `verso.log` (dossier utilisateur, run précédent dans
`verso.log.old`, module `verso-app/src/logging.rs`) reçoit chaque message de
la barre de statut, les avertissements de wgpu / winit / naga, et tout
plantage via un panic hook. La fenêtre Messages affiche son chemin.

Les dialogues de fichiers natifs bloquent leur thread ; ils tournent sur un
thread dédié (`verso-app/src/dialogs.rs`) pour que la sortie continue de
rendre pendant qu'un dialogue est ouvert (sauf macOS, qui impose le thread
principal).

## 5 bis. Espace de travail

Tous les panneaux (Clips, Composition, Group, Layer, Clip, Preview, Output,
Browser) sont des onglets d'un dock maison (`verso-app/src/dock.rs`, egui_dock
ne suivant pas egui 0.37) : glisser un onglet sur une barre ou le centre d'un
groupe l'y déplace, sur un bord il scinde le groupe ; les séparateurs se
redimensionnent ; les onglets se ferment et se rouvrent depuis le menu Window.
La disposition est enregistrée dans `<dossier utilisateur>/layout.ron` à la
fermeture.

Les panneaux Properties peuvent être multiples. Chacun suit la sélection
(🔗) ou reste verrouillé (🔒) sur un groupe, une couche ou un clip ; sa liste
déroulante choisit la cible directement (en mode suivi, elle change la
sélection). Un nouveau panneau ouvert depuis Window démarre verrouillé sur la
cible courante. Sélectionner un élément dans la grille met au premier plan le
panneau qui suit ce type d'élément.

## 6. Médias

| Format | Chemin de décodage | Phase |
|---|---|---|
| Images (PNG, JPEG, BMP, WebP, TGA, TIFF) | crate `image`, thread de chargement | v0.2 ✅ |
| Vidéo : HAP, H.264, H.265, ProRes, GIF… | processus `ffmpeg` → pipe RGBA, `-hwaccel auto` | v0.2 ✅ |
| HAP natif (upload BC1/BC3/BC7 sans conversion) | démuxage MOV + Snappy | plus tard |
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
lecture inversée sans coût. Les codecs inter-frames (H.264/H.265) ont un scrubbing
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

### Presets MilkDrop

Source `Source::Milkdrop { family, variant }` : un preset MilkDrop rendu
nativement (crate `verso-milkdrop` pour le CPU, `verso-render::milkdrop` pour
le GPU), porté de Butterchurn (MIT). Format lu : JSON Butterchurn (équations
EEL d'origine + shaders warp/comp déjà traduits de HLSL en GLSL).

- CPU : interpréteur EEL (ns-eel2 : division par zéro = 0, `megabuf`,
  `loop`/`while`, variables insensibles à la casse), équations par frame, par
  sommet (maillage 48×36), formes et ondes personnalisées, ondes de base
  (modes 0–7), vecteurs de mouvement, bordures. Audio : forme d'onde brute de
  l'entrée audio (1024 échantillons), FFT égalisée et niveaux bass/mid/treb
  relatifs à leur moyenne longue, comme MilkDrop.
- GPU : tampon de rétroaction RGBA8 par clip (clé couche, colonne), passe
  warp, chaîne de flou à 3 niveaux, sprites, passe comp vers la couche
  (converti en linéaire). Les passes internes inversent y pour garder l'ordre
  des lignes d'OpenGL ; les shaders des presets tournent donc sans retouche.
  Shaders GLSL enveloppés (prélude d'uniformes, textures de bruit, `texture`
  → `textureLod`) puis traduits en WGSL par naga.
- Familles : un fichier JSON regroupe les remixes d'un même preset
  (variantes). Le panneau du clip choisit la variante (◀ ▶, liste).
- Fondu MilkDrop : changer de variante garde le tampon de rétroaction et
  fond l'ancien preset vers le nouveau pendant `blend` secondes (2,5 par
  défaut, 0 = coupe). Motif de fondu par sommet tiré au hasard (balayage,
  plasma, radial), maillage warp mélangé, warp et comp du nouveau preset
  dessinés en fondu sur l'ancien, sprites des deux presets pondérés.
- Entrée : `input` désigne une autre couche dont l'image (convertie en
  couleurs d'affichage) sert de texture image au preset (`sampler_<nom>`,
  bruit sans entrée) ; `input_mix` la mélange dans la rétroaction à chaque
  image, que le preset déforme alors comme sa propre image (équivalent de
  l'entrée NDI/Spout de NestDrop). Mettre l'opacité de la couche source à 0
  pour ne l'utiliser que comme entrée ; une couche placée au-dessus arrive
  avec une image de retard.
- Bibliothèque : `presets/milkdrop` (147 familles, 206 presets, sélection
  Cream of the Crop de NestDrop croisée avec les presets Butterchurn, voir
  `presets/milkdrop/CREDITS.md`) et `%APPDATA%\Verso\milkdrop` pour les
  presets Butterchurn de l'utilisateur.

Compatibilité mesurée sur les 504 presets Butterchurn : 504 équations
compilent, 489 shaders traduisent (échecs : bugs de conversion d'origine).
Limites : pas de sprites ni de texte, import des `.milk` bruts (HLSL) non
géré, choix de variante et entrée non pilotables en MIDI/OSC pour l'instant. Outillage de sélection : `scripts/milkdrop`.

### Transitions

Durée par couche (0 = coupe). Au changement de clip, la dernière image affichée
est figée puis fondue vers le nouveau clip ; l'éjection fond vers le vide.
Limite actuelle : une vidéo sortante est figée sur sa dernière image pendant le
fondu au lieu de continuer à jouer. Seul le fondu enchaîné existe.

### Groupes

Un groupe peut contenir des couches et d'autres groupes (`Group::parent`). Une
couche appartient à son groupe le plus interne. Rendu récursif : chaque
groupe se compose dans sa propre paire de textures (une par profondeur), reçoit
ses effets puis se mélange dans son parent avec son blend, son opacité, son
bypass ; les opacités se multiplient. Le solo d'un groupe laisse passer toutes
ses couches.

Grille : en-tête coloré au-dessus de chaque groupe (flèche de repli, nom,
X, B, S, fader d'opacité, bouton ▶ par colonne qui ne lance que ce groupe),
bandes de couleur à gauche des couches selon l'imbrication, double-clic ou
flèche pour replier. Sélection multiple des couches par Ctrl/Maj-clic ;
`Ctrl+G` groupe la sélection (les couches doivent être adjacentes, un groupe
sélectionné en partie est inclus en entier et devient imbriqué) ;
`Ctrl+Shift+G` dissout le groupe courant, son contenu remonte d'un niveau.
OSC : `/group/N/column/M/launch`.

## 8. Contrôle et I/O

| Protocole | Usage | Phase |
|---|---|---|
| MIDI (`midir`) | Contrôleurs, MIDI learn | v0.3 ✅ |
| OSC (`rosc`) | Contrôle réseau, adresses stables par paramètre | v0.3 ✅ |
| Audio (`cpal` + `realfft`) | Niveaux par bande, beats, tempo détecté | v0.3 ✅ |
| Tap tempo, horloge en beats | Synchro des LFO | v0.3 ✅ |
| Scripts Lua (`mlua`, Lua 5.4) | Automations | v0.3 ✅ |
| Spout (Windows) / Syphon (macOS) | Partage de texture local | v0.4 |
| NDI® | Sortie vidéo réseau ✅ ; entrée à faire | v0.4 |
| Ableton Link | Synchro tempo | v0.4 |
| SMPTE / LTC | Timecode | v0.4 |
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

Compatibilité : `Action::parse_osc` accepte aussi le schéma
`/composition/layers/N/clips/M/connect`, `/composition/columns/M/connect`,
`/composition/layers/N/clear`, `/composition/layers/N/video/opacity`,
`/composition/master`, `/composition/crossfader/phase`,
`/composition/tempocontroller/tempo` (en BPM) et `/composition/disconnectall`,
documenté publiquement par les logiciels de VJ à grille, pour que les presets
OSC existants (TouchOSC, Chataigne, contrôleurs) pilotent Verso sans
modification. Seules les adresses des manuels publics sont reprises ; aucun
code tiers n'est consulté (voir README § OSC pour la table).

Les déclencheurs réagissent au passage au-dessus de 0,5 (le relâchement d'un
bouton ne fait rien). Port OSC par défaut : 7000, écoute activée depuis la
fenêtre Control.

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
Un script en erreur s'arrête jusqu'à sa prochaine modification.

### Sorties vers d'autres applications

Capture asynchrone de l'image de sortie (`FrameGrabber`, 3 buffers mappables) :
le rendu ne bloque jamais sur la relecture, l'image arrive avec 1 à 2 frames de
latence. Une sortie active seulement déclenche la capture.

NDI® : runtime chargé à l'exécution (`NDI_RUNTIME_DIR_V6`/`V5`), aucun SDK requis
pour compiler. Envoi RGBA sur un thread dédié, images sautées si l'envoi est en
retard. Vérifié par un aller-retour émetteur → récepteur NDI local en test.
Spout (Windows) et Syphon (macOS) restent à faire.

### Compteur de lancements

Chaque case (couche × colonne) compte ses lancements, quelle que soit la
source (clic, colonne, MIDI, OSC, script). Le compteur appartient à la case,
pas au média : remplacer le clip le conserve. Affiché sur la vignette et sur
le moniteur Preview, remis à zéro par clic droit. Enregistré avec la
composition.

## 9. Sorties et mapping (v0.5)

- Sortie plein écran par moniteur, fenêtre flottante.
- Slices : rectangle d'entrée dans la composition → quad de sortie.
- Warping : grille de Bézier par slice, edge blending, masques.
- Configuration de sortie séparée de la composition (dépend de la salle).

## 10. Feuille de route

| Version | Contenu | Critère de sortie |
|---|---|---|
| **v0.1 Core** | Grille couches × colonnes, sources génératives WGSL et couleurs, blend modes, opacité, master, preview, sortie plein écran, sauvegarde `.verso` | Un set de 30 min jouable au clavier/souris |
| v0.2 Médias ✅ | FFmpeg, HAP, images, effets ISF/WGSL sur clip/couche/groupe/composition, transitions, groupes | Lecture HAP 1080p × 4 couches à 60 fps (à mesurer) |
| v0.3 Contrôle ✅ | MIDI, OSC, learn, audio FFT/BPM, modulation (LFO, audio), Lua | Set piloté entièrement par contrôleur (à valider sur matériel) |
| v0.4 Interop | Spout, Syphon, NDI, Link, SMPTE | Échange avec TouchDesigner/OBS |
| v0.5 Mapping | Slices, warping, edge blending, multi-sorties | Mapping d'une scène à 2 projecteurs |

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
│   ├── verso-milkdrop/     presets MilkDrop : EEL, moteur, audio (sans GPU)
│   ├── verso-render/       moteur wgpu : sources, médias, blend, composition
│   │   ├── src/shaders/    WGSL des sources, blends et sortie
│   │   └── tests/          rendu GPU vérifié au pixel
│   └── verso-app/          application eframe/egui (binaire `verso`)
├── presets/milkdrop/       presets MilkDrop embarqués, par catégorie
├── scripts/milkdrop/       outillage de sélection des presets
└── docs/                   spécification, décisions
```
