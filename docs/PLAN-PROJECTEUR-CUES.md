# Plan — outils projecteur et cue list

Date : 2026-11-01. Statut : priorité validée, détail à relire avant le code.

Origine : comparaison avec MadMapper. On ne garde que ce qui complète
l'Advanced Output (AO) sans refaire ce que Resolume fait déjà. Les fonctions
3D sont notées à la fin pour plus tard ; elles ne sont pas développées
maintenant.

## 0. Ordre

| # | Étape | Taille | Dépend de |
|---|---|---|---|
| 1 | Highlight Selection dans l'AO | S | — (fait le 2026-11-01) |
| 2 | Rendu du soft edge, puis calcul automatique des recouvrements | M | — |
| 3 | Mires de test par écran | S | — |
| 4 | Identification des projecteurs | S | 3 |
| 5 | Cue list : modèle et commandes (`verso-core`) | M | — |
| 6 | Cue list : panneau, Go, fondus | M | 5 |
| 7 | Cue list : OSC, MIDI, raccourci clavier | S | 6 |

Les étapes 2–4 touchent `verso-core::output`, `verso-render::slices` et
l'éditeur AO. Les étapes 5–7 touchent la composition et une nouvelle crate
de panneau. Les deux pistes sont indépendantes.

## 1. Outils projecteur

### 1.1 Highlight Selection (fait)

Case « Highlight Selection » dans la barre de l'AO. Tant que la fenêtre AO
est ouverte, la slice sélectionnée est teintée en cyan avec des rayures
diagonales sur les écrans en sortie. On la retrouve sur le projecteur sans
couper l'image des autres slices.
Code : `ScreenRenderer::set_highlight`, `slice.wgsl` (`rgb.w`), test GPU
`highlighted_slice_is_tinted`.

### 1.2 Soft edge

Constat : les paramètres sont importés de Resolume (`SoftEdge` : power,
luminance, gamma ; `Slice::soft_edge`), mais **rien n'est dessiné**.
L'éditeur affiche « Soft edges are not drawn yet ».

1. **Modèle.** Ajouter à `Slice` une largeur de fondu par bord, en pixels de
   composition : `blend: [f32; 4]` (gauche, droite, haut, bas), `0` par
   défaut. Champ `#[serde(default)]`, les anciens fichiers s'ouvrent.
2. **Rendu.** Dans `slice.wgsl`, calculer la distance au bord en espace
   d'entrée (les UV de la slice) et appliquer la courbe :
   `f(x) = luminance · (2x)^power / 2` sous 0,5, `1 − luminance · (2(1−x))^power / 2`
   au-dessus, puis `f^(1/gamma)`. Les paramètres viennent de
   `OutputSetup::soft_edge`.
3. **Calcul automatique.** Bouton « Auto Soft Edge » : pour chaque paire de
   slices **sur des écrans différents** dont les rectangles d'entrée se
   recouvrent, la largeur du recouvrement devient le fondu des deux bords en
   regard. Rectangles alignés seulement au départ ; les autres formes sont
   listées dans le rapport (« à régler à la main »).
4. **Tests.** Unitaires dans `verso-core` : deux slices 0–1100 et 900–2000
   donnent 200 px à droite de la première et 200 px à gauche de la seconde.
   GPU : la colonne au milieu du fondu vaut environ la moitié de la luminance.

Note honnête : Arena sait aussi détecter les recouvrements. Ce n'est pas un
apport MadMapper, c'est un manque de l'AO actuel de Verso.

### 1.3 Mires de test

Un réglage par écran, `test_pattern: Option<Pattern>`, qui remplace le
contenu à la sortie. Elles sont dessinées par un shader procédural, sans
texture ni police :

- **Grille** : lignes tous les 100 px de l'écran, axes centraux, cercle
  inscrit, numéros de cases en chiffres 7 segments (SDF dans le shader).
  Sert à régler la géométrie et la mise au point.
- **Couleurs** : barres R, V, B, blanc, gris 50 %. Sert à égaliser les
  projecteurs (avec `ColorAdjust`).
- **Dégradé** : rampe horizontale noir → blanc. Sert à vérifier le soft edge.
- **Slices** : chaque slice remplie de sa couleur avec son numéro au centre.
  Montre ce que la composition envoie où.

Deux portées : un écran, ou tous les écrans (bouton dans la barre AO). La
mire reste active quand l'AO est en Live ; un bandeau rouge dans l'éditeur
le rappelle, pour ne pas rester en mire pendant un spectacle.

### 1.4 Identification

Bouton « Identify » : pendant 5 secondes, chaque écran affiche un grand
numéro (son rang dans la liste), un cadre de 20 px de sa couleur et sa
résolution. C'est la mire « Slices » plus un minuteur. On s'en sert au
branchement pour savoir quelle sortie va sur quel projecteur.

## 2. Cue list

But : une liste ordonnée d'états, que l'on rejoue avec un bouton Go, avec
fondu. Pour un spectacle rejoué chaque soir, et pour la formation (« cue 3 :
le refrain »). Resolume n'a que les colonnes.

### 2.1 Modèle (`verso-core`, sans GPU)

```rust
pub struct Cue {
    pub id: u64,
    pub name: String,
    pub state: CueState,
    pub fade: f32,            // secondes
    pub follow: Follow,       // Manual, After(secondes), Beats(n)
}
pub struct CueState {
    pub clips: Vec<Option<usize>>,   // colonne active par couche, None = éjectée
    pub opacity: Vec<f32>,           // par couche
    pub master: f32,
    pub crossfader: f32,
}
pub struct CueList { pub cues: Vec<Cue>, pub current: Option<usize> }
```

- `CueList` est enregistrée dans le `.verso` (`#[serde(default)]`).
- Les cues utilisent des **indices de couches**. Comme pour les modulateurs,
  `RemoveLayer` les renumérote. À reprendre quand `LayerId` existera
  (revue § 3, point 9).
- Plus tard : un look StageFlow et une séquence Chaser par cue, quand les
  phases B et C existeront.

### 2.2 Commandes

- Structure (dans l'annulation) : `RecordCue` (capture l'état courant),
  `UpdateCue`, `RemoveCue`, `MoveCue`, `SetCueFade`, `SetCueFollow`.
- Jeu (hors annulation, comme un fader) : `GoCue(index)`, `GoNext`,
  `GoPrevious`.
- Go déclenche les clips avec la transition de couche existante, réglée sur
  le fondu de la cue, et interpole opacités, master et crossfader. Le fondu
  avance dans la boucle de la composition (état d'exécution, non enregistré).
- `Follow::After` / `Beats` enchaîne seul la cue suivante, sur l'horloge de
  tempo existante.

### 2.3 Interface

Panneau dockable « Cues » :

- liste numérotée (nom, fondu, suite), cue courante en surbrillance, barre
  de progression du fondu ;
- gros bouton **Go**, plus Précédent et Record ;
- double-clic sur une cue = Go directement ;
- glisser pour réordonner.

### 2.4 Contrôle

- OSC : `/verso/cue/go`, `/verso/cue/next`, `/verso/cue/previous`,
  `/verso/cue/N`.
- MIDI learn sur Go, Précédent et chaque cue.
- Clavier : `Espace` = Go (convention théâtre). À vérifier : aucun raccourci
  actuel n'utilise Espace.
- MCP : outils `cue_record` et `cue_go`, pour tester sans souris.

### 2.5 Tests

Capture puis rejeu d'un état ; interpolation à mi-fondu ; renumérotation
après suppression d'une couche ; `Follow::After` qui enchaîne ; ancien
fichier sans cues qui s'ouvre.

## 3. Plus tard : fonctions 3D (pas de code maintenant)

Notées pour ne pas fermer de portes dans le modèle. Aucune n'est planifiée
avant la fin des étapes 1–7 et des phases A–C.

1. **Prévisualisation 3D de la scène** (moyen). Voir écrans et projecteurs
   en 3D. Réutiliser la scène de StageFlow (phase C : placement des slices
   sur une scène). Prérequis : phase C.
2. **Calibration par caméra** (gros). Une webcam filme des motifs projetés
   (codes de Gray, lumière structurée) et le mapping se fait seul. C'est de
   la vision par ordinateur : capture caméra, décodage, homographies.
   Prérequis : mires de test (§ 1.3), qui posent déjà le rendu de motifs.
3. **Mapping sur objet 3D** (gros). Charger un `.obj`, caler une caméra
   virtuelle sur le projecteur réel (points correspondants), projeter le
   contenu sur l'objet. Demande une passe de rendu 3D avec profondeur dans
   `verso-render`. Prérequis : 1.

Conséquence pour le modèle dès maintenant : garder des identifiants stables
(`u64`) pour écrans et slices, ce qui est déjà le cas. Aucun champ 3D n'est
ajouté tant que le besoin n'est pas là.
