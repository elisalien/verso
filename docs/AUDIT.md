# Verso — État des lieux, audit et plan (novembre 2026)

Audit du dépôt au commit `8804e79` (v0.3 + NDI + navigateur de shaders).
Méthode : lecture du code (~10 000 lignes Rust, 8 crates), compilation, tests,
clippy, et preuves de concept exécutées pour chaque faille marquée **[confirmé]**.

---

## 1. Checklist — où on en est

### Fait ✅

| Domaine | État |
|---|---|
| Moteur | Grille couches × colonnes, rendu wgpu `Rgba16Float` prémultiplié, 7 blend modes, master, crossfader A/B, solo/bypass |
| Sources | Générateurs WGSL, couleurs, images (`image`), vidéo FFmpeg en processus (HAP, ProRes, H.264…), shaders importés comme sources |
| Effets | 11 intégrés, WGSL maison + ISF (1 passe), chaînes sur clip / couche / groupe / composition, validation `naga` |
| Structure | Groupes de couches, transitions (fondu), compteur de lancements |
| Contrôle | OSC, MIDI + learn, analyse audio (bandes, beats, BPM), tap tempo, LFO / modulation audio |
| Scripts | Lua 5.4 (math/string/table/utf8), budget d'instructions |
| Interop | Sortie NDI (runtime chargé dynamiquement) |
| En ligne | Recherche/import ISF et ShaderToy, détection de licence |
| Fichier | Sauvegarde `.verso` (RON) |
| Qualité | Compilation sans warning, **clippy propre**, **87 tests verts** (2 ignorés : réseau), tests GPU au pixel |

### Pas encore fait ⬜

- Spout (Windows), Syphon (macOS), Ableton Link, SMPTE/LTC, entrée NDI (v0.4)
- Slices, warping, edge blending, multi-sorties (v0.5)
- HAP natif (upload BC sans conversion CPU), decks, identifiants stables (`LayerId`)
- Supprimer / déplacer / dupliquer couches, colonnes et clips ; annuler/rétablir
- CI, packaging/installeurs, journalisation, rapport de crash
- Critères de sortie v0.2/v0.3 « à mesurer / à valider sur matériel » : **jamais mesurés**

---

## 2. Sécurité

Surface d'attaque : fichiers `.verso` partagés (contiennent des **scripts Lua
exécutés automatiquement** et des chemins de médias), port OSC réseau, shaders
téléchargés, runtime NDI chargé dynamiquement.

| # | Gravité | Problème | Où |
|---|---|---|---|
| S1 | **Haute [confirmé]** | **Le chien de garde Lua est contournable avec `pcall`.** L'erreur « script took too long » est levée depuis le hook et rattrapée par un `pcall` du script : `while true do pcall(function() while true do end end) end` tourne indéfiniment (testé : > 60 s en release). Les scripts tournent sur le thread UI/rendu → **la sortie gèle**, en plein show, dès l'ouverture d'un `.verso` piégé. | `crates/verso-script/src/lib.rs:79-90` |
| S2 | **Haute [confirmé]** | **Pas de limite mémoire Lua.** `s=s..s` répété 31 fois alloue 2 Gio sans erreur (≈ 31 instructions, invisible pour le budget). Un peu plus → OOM / crash. | `sandbox()` |
| S3 | Moyenne [confirmé en debug] | `counter.get() * HOOK_STEP` déborde en `u32` : panique en debug, et en release le produit reboucle → le budget peut redevenir « sous la limite ». | `lib.rs:86` |
| S4 | Moyenne | OSC écoute sur `0.0.0.0` sans filtre : n'importe qui sur le réseau (Wi-Fi de la salle) peut piloter le show (`/clear`, `/master 0`). Le canal est non borné (inondation = mémoire). | `crates/verso-control/src/osc.rs:57` |
| S5 | Moyenne [confirmé] | **Valeurs NaN acceptées** : `clamp(NaN)` reste `NaN`. Un `/layer/1/opacity nan` ou `/bpm nan` en OSC (ou via script) corrompt l'état, se propage au GPU et est **sauvegardé** dans le fichier. | `command.rs`, `control.rs:67` |
| S6 | Moyenne | Les chemins média d'un `.verso` sont passés tels quels à `ffmpeg -i` : un fichier piégé peut utiliser les protocoles FFmpeg (`http://`, `concat:`, `subfile:`…) → requêtes réseau sortantes, lecture de fichiers locaux. Ajouter `-protocol_whitelist file,pipe` (ou vérifier que le chemin est un fichier existant). | `verso-media/src/player.rs:28`, `lib.rs:157,215` |
| S7 | Basse | Shaders importés : `naga` valide la syntaxe, pas la terminaison. Une boucle infinie GPU → TDR / perte du périphérique, et rien ne gère `device lost`. | `verso-render` |
| S8 | Basse | Clé API ShaderToy stockée en clair (fichier utilisateur, permissions par défaut). Préférer le trousseau OS (`keyring`) ou au moins `0600`. | `online_ui.rs:219` |
| S9 | Basse | NDI : `libndi` cherché aussi dans le répertoire courant / chemin de recherche par défaut (DLL planting sous Windows). Restreindre aux chemins absolus connus. | `verso-io/src/ndi.rs:111` |
| S10 | Info | Import collé : `id = "pasted{longueur du code}"` → deux shaders de même longueur s'écrasent. | `online_ui.rs:389` |

Points positifs : noms de fichiers d'import assainis (pas de path traversal),
`ffmpeg` lancé sans shell (pas d'injection de commande), `io`/`os`/`load`/`require`
retirés de Lua, limites de taille par défaut de `ureq` et `image`, code
`unsafe` limité au FFI NDI et documenté.

## 3. Stabilité

| # | Gravité | Problème | Où |
|---|---|---|---|
| T1 | **Haute [confirmé]** | **Un `.verso` incohérent fait planter l'appli.** Aucune validation après `from_ron` : une couche avec moins de cellules que de colonnes panique au lancement de colonne (`clips[column]`) et dès l'affichage de la grille (`grid.rs:309`). Idem `group` hors bornes (`panels.rs:131`), `width/height` énormes (création de texture → erreur wgpu → panique). | `composition.rs:175,420` |
| T2 | Haute | Pas de handler `on_uncaptured_error` ni `device lost` : toute erreur de validation wgpu **panique** (comportement par défaut). | `verso-render` |
| T3 | Haute | Aucun hook de panique ni journal : en release Windows (`windows_subsystem = "windows"`) un crash ferme la fenêtre sans aucune trace. | `main.rs` |
| T4 | Moyenne | Sauvegarde non atomique (`fs::write` direct) : un crash pendant l'écriture détruit le projet. Pas d'autosave, pas de `.bak`. | `app.rs:326` |
| T5 | Moyenne | Scripts, décodage des messages, analyse et rendu partagent le thread UI : toute lenteur UI (dialogue fichier `rfd` **bloquant**, gros panneau) fait sauter des frames de sortie. | architecture v0.1 |
| T6 | Basse | `fps` issu de `ffprobe` : `0/0` ou valeur aberrante → vidéo figée ; `width*height*4` en `u32` (borné aujourd'hui par `fit_size`, fragile). | `verso-media` |
| T7 | Basse | Pas de CI : les tests GPU / FFmpeg / réseau ne tournent que sur la machine du dev. | — |

## 4. UX / UI — revue, QoL et bonnes pratiques

### Risques en live (à traiter en priorité)

1. **`Échap` vide toutes les couches.** C'est la touche réflexe pour fermer un
   menu ou un dialogue → écran noir accidentel. Proposer `Maj+Échap` ou un
   double appui, et l'option « panic = fondu au noir » plutôt que coupe.
2. **Cliquer une cellule la lance.** Impossible de sélectionner/éditer un clip
   sans l'envoyer en sortie. Bonne pratique VJ : clic = sélection, double-clic
   ou clic sur la zone « play » = lancer (option « clic lance » pour ceux qui
   préfèrent), ou modificateur (`Ctrl+clic` = sélectionner sans lancer).
3. **Pas de confirmation** sur Nouveau / Ouvrir / fermeture avec modifications
   non enregistrées, ni sur « Clear » d'une cellule.
4. **Pas d'annuler/rétablir.** Le modèle par `Command` s'y prête bien
   (pile d'inverses ou snapshots RON).

### QoL manquants

- Raccourcis standards : `Ctrl+S`, `Ctrl+Maj+S`, `Ctrl+O`, `Ctrl+N`, `Ctrl+Z/Y`,
  `Suppr` sur la cellule sélectionnée, `Ctrl+C/V/D` sur les clips.
- Glisser-déposer cellule → cellule (déplacer ; `Alt` = copier), réordonner
  couches et colonnes, **supprimer** couche/colonne (aucune commande n'existe).
- Fichiers récents, réouverture du dernier projet, autosave + récupération.
- Indicateur « modifié » (`●` dans le titre), barre de statut avec FPS de
  rendu, frames sautées, charge des décodeurs.
- Mémoriser taille/position des fenêtres et la sortie plein écran par écran.
- Raccourcis colonnes au-delà de 9 (pages, ou `Maj+1..9` = colonnes 10–18).
- Médias manquants à l'ouverture : dialogue « relocaliser le dossier ».
- Aperçu (preview) d'un clip sans l'envoyer en sortie (bus « cue »).
- Indiquer clairement l'état OSC/MIDI/audio (LED d'activité) dans la barre.

### Bonnes pratiques d'interface

- **Infobulles** : seulement 18 `on_hover_text` ; en mettre sur toutes les
  icônes et lettres (`B`, `S`, `X`, `A/B`) avec leur raccourci.
- **Accessibilité** : vérifier le contraste de `TEXT_DIM (#82868C)` sur
  `BG_PANEL (#1C1E21)` (≈ 4,6:1, limite) ; ne pas coder l'état uniquement par
  la couleur (solo jaune / bypass) → ajouter forme ou texte ; taille de police
  réglable (`interact_size.y = 18` est petit sur écran 4K ou en salle sombre).
- **Retour d'erreur** : erreurs d'effet, de média et de script regroupées dans
  un panneau « Problèmes » plutôt que seulement dans la barre de statut.
- **Cohérence** : verbes identiques dans menus, clics droits et doc ; menus
  désactivés plutôt que masqués ; actions destructives en rouge, en bas.
- **Mode Live/Verrouillé** : bascule qui désactive l'édition (glisser-déposer,
  suppression) pendant le show.

## 5. Limites actuelles (estimations)

| Sujet | Limite | Origine |
|---|---|---|
| Vidéo | ≈ 250 Mo/s de copie CPU par couche 1080p30 ; ~4 couches 1080p60 = ~2 Go/s de copie + upload → limite réaliste **3–4 couches 1080p60**, 1–2 en 4K, selon CPU | FFmpeg en processus, RGBA via pipe |
| Scrubbing / vitesse | Seek = relance de `ffmpeg` (100–500 ms) ; pas de lecture arrière | idem |
| Transitions | Vidéo sortante figée pendant le fondu ; fondu seul | `verso-render` |
| Effets | 32 scalaires par effet, ISF une passe, une image d'entrée | uniforms 8×`vec4` |
| Fluidité | Rendu sur le thread UI : dialogues et UI lourde font sauter des frames | eframe v0.1 |
| Mappings | Indices, pas d'ID stables : insérer/déplacer une couche décalera les mappings MIDI/OSC et scripts | `verso-core` |
| Sorties | Une fenêtre ; NDI avec relecture CPU (1–2 frames de latence) | — |
| Plateformes | Linux : dépend d'ALSA (`libasound2-dev` requis pour compiler, non documenté) | `cpal` |
| Taille | 16–8192 px de composition (limite GPU par défaut 8192, parfois 4096/2048 sur iGPU) | wgpu |

## 6. Plan des prochaines mises à jour

### v0.3.1 — Correctifs sécurité & stabilité (≈ 1 semaine, prioritaire)

1. Lua : `set_memory_limit` (ex. 64 Mio/script), budget en `u64` avec
   saturation, **erreur non rattrapable** (retourner `VmState::Yield`/poser un
   drapeau « tué » vérifié à chaque hook et refuser de reprendre ; ou
   remplacer `pcall`/`error` par des versions qui relancent l'erreur
   d'interruption) + tests de régression (`pcall`, `string.rep`, concat).
2. Validation à l'ouverture (`Composition::validate`/`repair`) : longueurs
   `clips`/`launches` = colonnes, `active`/`group` dans les bornes, taille
   bornée, valeurs finies ; test de fuzz (`cargo fuzz` ou proptest) sur `from_ron`.
3. Rejeter les non-finis dans `Command::apply` et `Range::from_unit`.
4. FFmpeg : `-protocol_whitelist file,pipe` + chemin = fichier existant.
5. `on_uncaptured_error` (journaliser, désactiver l'effet fautif), gestion
   `device lost`, hook de panique écrivant un `crash.log`.
6. Sauvegarde atomique (fichier temporaire + `rename`) + `.bak`.
7. OSC : choix de l'interface (127.0.0.1 par défaut), liste d'IP autorisées,
   canal borné.

### v0.3.2 — QoL & sûreté live (≈ 2 semaines)

- Échap protégé, clic = sélection / lancement configurable, mode Live verrouillé.
- Annuler/rétablir, confirmation des modifications non enregistrées, autosave.
- Raccourcis standards, fichiers récents, indicateur « modifié ».
- Supprimer / déplacer / dupliquer couches, colonnes, clips (et passage aux **ID
  stables** pour que mappings et scripts suivent).
- Infobulles partout, panneau « Problèmes », barre de statut FPS/frames sautées.
- CI GitHub Actions (Linux/Windows/macOS : build, clippy, tests hors GPU) et
  `cargo deny`/`cargo audit` ; documenter `libasound2-dev`.

### v0.4 — Interop (≈ 4–6 semaines)

- Thread de rendu séparé de l'UI (déjà préparé par `Command`/`Snapshot`) —
  prérequis pour Spout/Syphon et la fluidité.
- Spout (DX11 partagé via hooks HAL wgpu), Syphon (Metal), entrée NDI,
  Ableton Link, SMPTE/LTC.
- Banc de mesure : vérifier enfin « HAP 1080p × 4 couches à 60 fps ».

### v0.4.x — Performance médias

- HAP natif (démuxage MOV + Snappy → textures BC), file de frames avec
  budget mémoire, seek sans relance, lecture arrière, vidéo sortante qui
  continue pendant la transition.

### v0.5 — Mapping

- Slices, warping Bézier, edge blending, masques, multi-sorties, config de
  sortie séparée de la composition.

### Ensuite

- Packaging (installeur Windows signé, `.app` notarisé, AppImage/Flatpak),
  mises à jour, documentation utilisateur, decks, DMX/Art-Net.

---

## 7. Suivi

### v0.3.1 — fait

| # | Correction | Test |
|---|---|---|
| S1 | `pcall`/`xpcall` remplacés : une fois le budget dépassé, l'erreur est relancée hors de l'appel protégé | `pcall_cannot_catch_the_instruction_budget` |
| S2 | `set_memory_limit(64 Mio)` par script | `memory_is_limited` |
| S3 | Compteur `u64` saturant | — |
| S4 | OSC sur 127.0.0.1 par défaut, option réseau + liste d'IP, file bornée (4096), paquets trop imbriqués refusés, compteur de refus | `deeply_nested_bundles_are_dropped`, `senders_outside_the_allow_list_are_ignored` |
| S5 | `Command::is_finite` : commandes non finies refusées ; `Action::command` ignore NaN | `non_finite_commands_are_refused` |
| S6 | Entrée FFmpeg : fichier existant, préfixe `file:`, `-protocol_whitelist file` | `only_local_files_are_opened` |
| S8 | Clé ShaderToy en `0600` (Unix) | — |
| S9 | NDI : chemins absolus seulement sous Windows (+ dossier de l'exécutable) | — |
| S10 | Id de code collé = hash FNV du code | — |
| T1 | `Composition::repair` après chaque ouverture (cellules, indices, tailles, valeurs) ; réparations affichées dans la barre d'état | `inconsistent_files_are_repaired` |
| T2 | `on_uncaptured_error` + `device lost` : journalisés et affichés au lieu de paniquer ; taille de rendu bornée à la limite du GPU | — |
| T3 | Hook de panique → `crash.log` (backtrace) + `verso.log` | — |
| T4 | Sauvegarde atomique (`.tmp` + `rename`) et `.bak` | — |
| T6 | FPS aberrant → 30 ; tailles de frame en `usize` | — |

Restent : S7 (boucle infinie dans un shader GPU, non détectable avant
exécution — atténué par T2) et T5 (thread de rendu séparé, prévu en v0.4).

### v0.3.2 — fait

- **Sûreté live** : `Échap` seul ne vide plus rien (`Maj+Échap`) ; préférence
  « clic lance » ou « clic sélectionne / double-clic lance », `Ctrl+clic`
  sélectionne toujours sans lancer ; **verrou live** (`Ctrl+Maj+L`) qui ne
  laisse passer que lancements et contrôles de jeu.
- **Annuler/rétablir** (`Ctrl+Z`, `Ctrl+Maj+Z`/`Ctrl+Y`, menu Edit avec le nom
  de l'action) : 100 pas, drags fusionnés, lancements/MIDI/OSC/scripts exclus,
  ce qui joue n'est jamais changé par une annulation.
- **Modifications non enregistrées** : `*` dans le titre et la barre,
  confirmation à Nouveau / Ouvrir / Fermer, copie de récupération chaque minute
  proposée au démarrage après un crash.
- **Structure** : supprimer, déplacer, dupliquer couches ; supprimer, déplacer
  colonnes ; glisser une case sur une autre (échange, `Alt` copie) ; copier /
  couper / coller / effacer une case. Mappings MIDI et modulateurs suivent
  (`IndexChanges`), état de rendu et lecteurs vidéo aussi.
- **QoL** : raccourcis standards, fichiers récents, réouverture du dernier
  projet, `Maj+1–9` pour les colonnes 10–18, relocalisation des médias manquants,
  panneau Problèmes (⚠), préférences (taille d'interface 75–200 %), fenêtre
  mémorisée (eframe `persistence`), AccessKit activé.
- **Accessibilité** : `TEXT_DIM` passé à `#979BA1` (contraste ≥ 5:1 sur tous les
  fonds), infobulles sur cellules et en-têtes avec leurs raccourcis.
- **Bug corrigé** : afficher un clip couleur/générateur envoyait une édition
  fantôme (aller-retour 8 bits du sélecteur de couleur).
- **CI** : GitHub Actions Linux/Windows/macOS (fmt, clippy `-D warnings`,
  tests ; GPU logiciel lavapipe sous Linux) + `cargo deny` (failles, licences,
  sources). `VERSO_TRACE=1` affiche chaque commande.

### v0.4 — fait

- **Dialogues de fichiers non bloquants** (`rfd` asynchrone sondé à chaque
  frame) : la sortie ne gèle plus pendant Ouvrir / Enregistrer / dossiers.
- **NDI® entrée** : source de clip `Ndi`, recherche des émetteurs dans
  Browser › Sources, un récepteur par couche sur son thread.
- **Spout** (Windows) et **Syphon** (macOS) en sortie, sans SDK à la
  compilation. Vérifiés par compilation croisée (`x86_64-pc-windows-gnu`,
  `aarch64-apple-darwin`) ; **pas testés sur machine** faute de Windows/macOS
  ici : à valider avant un show.
- **Ableton Link** : tempo et phase partagés (deux pairs testés en local).
- **Timecode** : MTC et LTC (décodeur testé par aller-retour encodeur →
  décodeur, 25 et 30 fps, passage d'heure, bruit rejeté), cues de timecode
  enregistrées dans la composition, `t.timecode` dans Lua.
- **Banc de mesure** : `cargo run --release -p verso-app --example bench`.
  Sur ce conteneur (4 cœurs, GPU logiciel llvmpipe, non représentatif) :
  décodage FFmpeg 1080p ≈ 185 fps H.264 / 122 fps HAP pour un lecteur, mais
  ≈ 36 fps par lecteur à 4 en parallèle → le chemin FFmpeg→RGBA est le goulot,
  ce qui confirme la priorité du HAP natif (v0.4.x).

**Reporté** : séparer le rendu du thread UI. La fenêtre de sortie est une
vue egui dessinée par le thread UI ; un thread de rendu seul ne la libère pas.
Il faut une fenêtre de sortie avec sa propre surface wgpu présentée hors du
thread UI, à faire avec le mapping multi-sorties (v0.5), où chaque sortie aura
sa surface. Les dialogues bloquants, principale cause de gel, sont réglés.
