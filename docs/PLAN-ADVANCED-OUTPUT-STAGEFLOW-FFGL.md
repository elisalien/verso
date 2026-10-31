# Plan — Advanced Output, compatibilité Chaser / StageFlow, hôte FFGL

Date : 2026-10-31. Statut : proposition, à valider avant tout code.

## 0. Décisions de cadrage

1. **Ordre.** Advanced Output (slices) d'abord. Chaser et StageFlow ensuite,
   car les deux reposent entièrement sur les slices. FFGL est une piste
   indépendante (nouvelle crate) qui peut avancer en parallèle.
2. **Pas d'IPC Resolume, pas de plugins Chaser/StageFlow.** Verso ne parle
   pas aux applications Chaser et StageFlow et ne charge pas leurs plugins.
   Verso **importe leurs documents** (`.chase`, `.flow`) et **réimplémente
   leur comportement en natif**. Les documents sont du XML lisible appartenant
   à l'utilisateur ; on les lit, on ne désassemble rien. Même chose pour les
   presets Advanced Output de Resolume (`.xml`).
3. **Amendement de la SPEC.** Le principe clean-room de la SPEC dit « aucun
   format propriétaire repris ». Proposition de reformulation : *le format
   natif de Verso reste propre ; des importeurs en lecture seule existent pour
   les formats tiers documentés par l'usage (fichiers XML de l'utilisateur).
   Aucun binaire tiers n'est désassemblé, aucun protocole privé n'est
   reproduit.* La phrase « FFGL n'est pas visé » de la SPEC § 7 est remplacée
   par la section FFGL ci-dessous.
4. **Revue du 31/10.** La revue propose quatre semaines de stabilisation avant
   toute nouvelle fonctionnalité. Ce plan ne la remplace pas : il commence
   après, ou en accepte le report en connaissance de cause.

## 1. Ce que contiennent les fichiers fournis

### Preset Advanced Output (`HSS 4 ecrans.xml`)

- Toile de composition : `CurrentCompositionTextureSize` 1920 × 1920.
- `Screen` (id `uniqueId`, nom, `Enabled`, `Hidden`, réglages couleur,
  `OutputDevice`) contient des `Slice`.
- `Slice` : `InputRect` (4 points en pixels de composition), `OutputRect`
  (4 points en pixels d'écran), `Warper` avec `BezierWarper`
  (grille `controlWidth × controlHeight`, ici 4 × 4) et `Homography`,
  `Flip`, réglages couleur, `SoftEdgeEnable`, `Input Source`.
- `SoftEdging` au niveau écran (`Power`, `Luminance`, `Gamma`).

### Chaser (`.chase`)

- `<screens>` recopie le preset AO : mêmes `uniqueId` de slices, chemins
  `m x y l … z` (polygones, ici des rectangles).
- `<sequences>` : chaque séquence a des `step`, chaque step liste les slices
  **allumées**. Les autres slices sont masquées (noir) pendant ce step.
- `appState` : séquence et step actifs. `views` : purement UI.
- Exemple : Séquence 1 balaie les 4 écrans un par un ; Séquence 2 alterne
  extérieurs / intérieurs.

### StageFlow (`.flow`)

- `<arenaScreens>` : mêmes slices, avec en plus une `shape` placée sur une
  **scène** (stage) par une matrice affine `t="a,b,tx,c,d,ty"`. Les 4 écrans
  sont posés côte à côte en x = 160, 560, 960, 1360, y = 560 (400 × 800 chacun).
- `<looks>` : chaque `Look` a une taille de contenu (`x`, `y`) et des
  **focals**. Une focal est une zone de la scène (shape + transform) où le
  contenu du look est dessiné, avec des options (`flip`, `mirror`,
  `distribution`, `extend`, `cover`, `twin`, `excluded`).
- Modèle déduit, à confirmer par comparaison visuelle avec StageFlow : les
  transforms des focals ont un décalage constant de 45 380 px sur x et y par
  rapport aux slices. Avec ce décalage :
  - « Scene Full » : une focal 1600 × 800 couvre exactement les 4 écrans.
  - « Paires » : deux focals 800 × 800, la droite en miroir (`flip="1"`).
  - « Tableau G/D » : contenu portrait 1080 × 1920 réduit à 450 × 800,
    centré sur un écran 400 de large (bords coupés).
  - « Each » : le même contenu portrait centré sur chacun des 4 écrans.
- Rendu d'un look : pour chaque slice, prendre la zone de scène qu'elle
  occupe, y lire ce que les focals y dessinent, et écrire le résultat dans la
  zone `inputPath` de la slice dans la composition. L'Advanced Output envoie
  ensuite cette zone vers l'écran physique.

## 2. Phase A — Advanced Output (v0.5 avancée)

Objectif : reproduire le flux « composition → slices → écrans » et charger le
preset `HSS 4 ecrans.xml` sans retouche.

### A1. Modèle (`verso-core`, sans GPU)

```rust
pub struct OutputSetup {          // fichier séparé : *.versoout (SPEC § 9)
    pub canvas: (u32, u32),       // toile de composition, ex. 1920×1920
    pub screens: Vec<Screen>,
}
pub struct Screen {
    pub id: u64,                  // conserve les uniqueId importés
    pub name: String,
    pub enabled: bool,
    pub target: ScreenTarget,     // Window { monitor }, Ndi { name }, Virtual
    pub resolution: (u32, u32),
    pub color: ColorAdjust,       // opacité, luminosité, contraste, RVB
    pub soft_edge: SoftEdge,
    pub slices: Vec<Slice>,
}
pub struct Slice {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub input: Vec<[f32; 2]>,     // polygone en pixels de composition
    pub output: Warp,             // Quad(4 points) ou Bezier { w, h, points }
    pub flip: Flip,
    pub color: ColorAdjust,
    pub soft_edge: bool,
}
```

- Identifiants `u64` stables : indispensables pour Chaser et StageFlow, qui
  référencent les slices par id. Ils répondent aussi au point « identifiants
  stables » de la revue.
- Commandes : `AddScreen`, `AddSlice`, `SetSliceInput`, `SetSliceOutput`,
  `SetScreenTarget`… via le système `Command` existant (annulation incluse).
- Tests unitaires : sérialisation, ids uniques, géométrie (homographie
  quad → quad, inversion).

### A2. Rendu (`verso-render`)

- Une texture par écran (résolution de l'écran).
- Par slice, un maillage : positions = points de sortie, UV = points d'entrée
  divisés par la toile. Quad → quad avec correction perspective (homographie,
  coordonnée `q` dans le vertex). Bézier : grille subdivisée (ex. 16 × 16)
  évaluée sur le CPU quand les points changent.
- Polygones d'entrée non rectangulaires : triangulation (`earcutr`) ;
  rectangles d'abord.
- Flip, réglages couleur et soft edge dans le shader de slice.
- Test GPU au pixel dans `crates/verso-render/tests/render.rs` : une slice
  400 × 800 recopiée et retournée.

### A3. Sorties

- `Window` : une fenêtre egui par écran actif, plein écran sur un moniteur
  choisi (borderless fullscreen sur un moniteur donné ; sous Wayland c'est le
  seul placement fiable, à tester sur Fedora).
- `Ndi` : un `NdiWorker` + `FrameGrabber` par écran (code de `outputs.rs`
  généralisé).
- `Virtual` : écran sans sortie, utile pour un écran « TD MOD IN » ou NDI à
  résolution libre.
- L'actuelle « Output Window (F2) » devient un écran par défaut
  (une slice plein cadre), donc aucune régression.

### A4. Éditeur

- Panneau « Advanced Output » : liste écrans/slices, deux vues
  (Input sur la composition, Output sur l'écran), poignées de coins,
  grille Bézier, aimantation, saisie numérique.
- Identification : afficher nom et couleur de slice sur la sortie à la demande.

### A5. Import Resolume Advanced Output

- `verso-io::import::resolume_ao` : lit `Screen`, `Slice`, `InputRect`,
  `OutputRect`, `BezierWarper`, `Flip`, couleurs, soft edge.
- `OutputDevice` ne se traduit pas tel quel : l'utilisateur rechoisit le
  moniteur ou le nom NDI à l'import.
- Ignoré au départ (signalé dans un rapport d'import) : masques, guides,
  `Input Source` autre que composition, `Is Key`.
- Test : le preset `HSS 4 ecrans.xml` copié dans `tests/fixtures/`
  (le fichier appartient à l'utilisateur).

Critère de sortie : le preset HSS se charge, 4 slices sur l'écran NEON,
écran NDI et écran TD actifs, mapping à 2 projecteurs jouable (critère v0.5).

## 3. Phase B — Chaser natif

### B1. Modèle

```rust
pub struct ChaseSequence { pub id: u64, pub name: String, pub steps: Vec<Vec<u64>> } // ids de slices
pub struct ChaseState { pub sequence: usize, pub step: usize, pub mode: ChaseMode }
pub enum ChaseMode { Beat { per_step: f32 }, Manual, Random }
```

Les séquences appartiennent à l'`OutputSetup` (elles dépendent des slices).

### B2. Effet « Slice Chaser »

- Effet natif (Rust + WGSL), posable sur clip, couche, groupe ou composition.
- À chaque changement de step, rasteriser les polygones d'entrée des slices
  allumées dans une texture masque (taille composition). Le shader multiplie
  par le masque. Les 32 paramètres scalaires d'un effet ne suffisent pas pour
  des polygones, d'où la texture masque.
- Avance : tempo (horloge existante, Link plus tard), déclenchement manuel,
  MIDI/OSC (adresses `/verso/chaser/step`, `/verso/chaser/sequence`),
  modulation comme les autres paramètres.
- Paramètres : séquence (choix), step (int), fondu entre steps, inversion.

### B3. Import `.chase`

- `verso-io::import::chaser` : lit `<screens>` (vérifie que les ids
  correspondent à l'`OutputSetup` ; sinon propose d'importer aussi les
  slices), puis `<sequences>`.
- `views`, `PixelMap` : ignorés.
- Test : le fichier fourni donne 3 séquences, steps 4/2/4.

Critère de sortie : la Séquence 3 du fichier fourni tourne au tempo sur les
4 écrans du preset HSS.

## 4. Phase C — StageFlow natif

### C1. Modèle

```rust
pub struct Stage { pub placements: Vec<(u64, Affine2)> }        // slice → scène
pub struct Look { pub id: u64, pub name: String, pub size: (u32, u32), pub focals: Vec<Focal> }
pub struct Focal { pub shape: Vec<[f32; 2]>, pub transform: Affine2,
                   pub flip: bool, pub mirror: bool, pub excluded: Vec<u64> }
```

### C2. Effet « Look »

- Effet natif posable sur clip ou couche. Entrée : la texture de la couche,
  traitée comme toile du look (`size`). Sortie : texture composition.
- Pour chaque slice non exclue : maillage dont les UV viennent de
  l'intersection slice × focal dans l'espace scène, puis transformées vers la
  toile du look ; positions = `input` de la slice dans la composition.
- Paramètre « look » (choix) + transition entre looks (fondu, réutilise le
  système de transitions).
- Ordre d'implémentation des options : `flip` (présent dans le fichier),
  puis `mirror`, `excluded`. `distribution`, `extend`, `cover`, `twin` sont
  tous à 0 dans le fichier : comportement à définir par observation, en
  phase ultérieure.

### C3. Import `.flow`

- `arenaScreens` → placements sur la scène ; `looks` → looks.
- Décalage des focals (45 380 px observé) : calculé, pas codé en dur. Valider
  sur 2 ou 3 looks en comparant une capture StageFlow et une capture Verso.
- Test : 5 looks, focals et flips du fichier fourni.

Critère de sortie : les 5 looks du fichier fourni donnent la même image que
StageFlow sur le preset HSS, à l'œil, avec un clip de test à grille numérotée.

## 5. Phase D — Hôte FFGL

### D1. Ce que FFGL permet, et ce qu'il ne permet pas

- L'API FFGL 2 est publique (SDK open source de Resolume sur GitHub ; licence
  à relire avant de lier quoi que ce soit). Héberger des plugins FFGL tiers
  est légitime, sans rétro-ingénierie.
- Les effets intégrés de Resolume ne sont pas des plugins réutilisables
  hors de Resolume (internes, ou soumis à leur licence). « Compatibilité avec
  les effets Resolume » signifie donc : **les plugins FFGL tiers** que les VJ
  installent aussi dans Resolume. Pour les effets intégrés, la voie reste
  la réimplémentation WGSL/ISF.
- Les plugins Chaser et StageFlow sont des FFGL, mais ils dépendent de l'IPC
  privé de leur application. Les héberger ne donne pas Chaser ni StageFlow :
  ce sont les phases B et C.
- **Fedora** : presque tous les plugins FFGL sont distribués en `.dll`
  (Windows) et `.bundle` (macOS). Sous Linux, l'offre est quasi nulle. FFGL
  sert donc surtout la build Windows ; c'est à accepter explicitement vu la
  cible Fedora.

### D2. Architecture recommandée : hôte hors processus

```
verso (wgpu)  ──frames RGBA (mémoire partagée)──▶  verso-ffgl-host (OpenGL)
              ◀──frames traitées────────────────   charge les .dll FFGL
              ──commandes (params, temps, BPM)──▶
```

- Nouveau binaire `verso-ffgl-host` + crate `verso-ffgl` (côté Verso).
- Raisons : un plugin qui plante ou fuit ne tue pas le rendu (principe
  « le rendu ne bloque jamais ») ; le contexte OpenGL ne cohabite pas avec
  wgpu dans le même processus ; un plugin bloqué est tué et relancé.
- Côté hôte : contexte GL 4.1 core caché (`glutin` + `glow`), chargement
  `libloading`, appels `plugMain` (`FF_INSTANTIATEGL`, `FF_PROCESSOPENGL`,
  `FF_SETPARAMETER`, `FF_SETTIME`, `FF_SETBEATINFO`…), FBO d'entrée/sortie.
- Transport v1 : copie CPU. Verso relit la frame (le `FrameGrabber`
  existant), l'écrit en mémoire partagée ; l'hôte l'envoie en texture GL
  (PBO), exécute la chaîne, relit (PBO), Verso téléverse le résultat.
  Coût estimé en 1080p RGBA8 : 8 Mo par sens, 1 à 2 frames de latence.
  Mesurer avant de promettre 60 fps.
- Transport v2 (optionnel, seulement si la mesure l'exige) : zéro copie via
  handles partagés. Vulkan + `GL_EXT_memory_object_win32/fd` + sémaphores
  partagés, importés dans wgpu par les hooks HAL. Gros chantier, réservé à
  l'après-v1.

### D3. Intégration dans Verso

- Type d'effet « FFGL » à côté de WGSL et ISF : paramètres lus via
  `FF_GETPARAMETERNAME/TYPE/DEFAULT`, mappés sur le modèle de paramètres
  (donc MIDI, OSC, modulation gratuits).
- Les plugins FFGL « source » (générateurs) deviennent des sources de clip.
- Placement : clip, couche, groupe, composition. Chaque instance FFGL coûte
  un aller-retour : afficher le coût dans l'UI et le limiter par défaut.
- Liste de compatibilité testée (comme pour ISF) : 5 à 10 plugins gratuits
  connus, Windows d'abord.

Critère de sortie : un plugin FFGL tiers gratuit tourne sur une couche
1080p à 60 fps sous Windows, et un plugin qui plante n'arrête pas la sortie.

## 6. Découpage et ordre

| # | Étape | Dépend de | Taille |
|---|---|---|---|
| A1 | Modèle OutputSetup + ids stables | — | M |
| A2 | Rendu slices (quad, homographie, Bézier) | A1 | L |
| A3 | Écrans fenêtre / NDI / virtuel | A2 | M |
| A5 | Import preset AO Resolume | A1 | S |
| A4 | Éditeur Advanced Output | A2 | L |
| B | Chaser natif + import `.chase` | A1–A3, A5 | M |
| C | StageFlow natif + import `.flow` | A1–A3, A5 | L |
| D0 | Spike FFGL : un plugin, copie CPU, mesure | — | M |
| D | Hôte FFGL complet | D0 | L |

D0 peut commencer dès maintenant en parallèle : il ne touche aucun fichier
de la phase A. Le spike tranche la question coût/latence avant d'investir.

## 7. Risques

| Risque | Mitigation |
|---|---|
| Interprétation StageFlow (décalage, options de focal) inexacte | Comparaison visuelle avec une grille numérotée ; options non vues = non supportées, signalées à l'import |
| Placement plein écran par moniteur sous Wayland | Borderless fullscreen sur moniteur choisi ; test Fedora tôt (A3) |
| Coût de l'aller-retour FFGL | Spike D0 et mesure avant D ; zéro copie seulement si nécessaire |
| Plugins FFGL indisponibles sous Linux | Fonction Windows d'abord, documentée comme telle |
| Formats tiers changent de version | Importeurs tolérants, versions testées notées (Arena 7.28, Chaser 4.4, StageFlow 1.6) |
| Taille du chantier pour un mainteneur solo | Phases livrables une à une ; A seule apporte déjà le mapping v0.5 |
