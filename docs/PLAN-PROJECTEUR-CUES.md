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
| 2 | Rendu du soft edge, recouvrements calculés automatiquement | M | — (fait le 2026-11-01) |
| 3 | Mires de test par écran | S | — (fait le 2026-11-01) |
| 4 | Identification des projecteurs | S | 3 (fait le 2026-11-01) |
| 5 | Cue list : modèle et commandes (`verso-core`) | M | — (fait le 2026-11-01) |
| 6 | Cue list : panneau, Go, fondus | M | 5 (fait le 2026-11-01) |
| 7 | Cue list : OSC, MIDI, raccourci clavier | S | 6 (fait le 2026-11-01) |

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

### 1.2 Soft edge (fait)

Avant : les paramètres étaient importés de Resolume (`SoftEdge` : power,
luminance, gamma ; `Slice::soft_edge`), mais rien n'était dessiné.

Choix final, identique à Arena : pas de largeur à saisir. Chaque slice qui a
« Soft Edge » activé se fond avec toutes les autres slices à soft edge dont
l'entrée la recouvre, sur n'importe quel écran. Le recouvrement est donc
calculé automatiquement, à chaque image, et le format Resolume reste
compatible à l'export.

- **Poids.** En un point de la composition, chaque slice pèse sa distance au
  bord de sa propre entrée, divisée par la somme de ces distances sur toutes
  les slices qui couvrent ce point. Les poids font 1 au total, la rampe
  occupe tout le recouvrement, et cela marche à trois projecteurs ou plus.
- **Courbe.** Courbe en S de `power` passant par (0,5 ; `luminance`), puis
  exposant `gamma × gamma RVB / 2`, appliquée à la lumière (espace linéaire).
  Avec les valeurs par défaut, deux projecteurs donnent ensemble la pleine
  lumière.
- **Limites.** 64 slices à soft edge au plus (au-delà, non fondues, signalé
  dans l'éditeur). Les polygones utilisent le rectangle englobant de leur
  entrée.
- **Code.** Référence CPU testée dans `verso-core::output` (`edge_distance`,
  `blend_weight`, `SoftEdge::curve`), calcul par pixel dans `slice.wgsl`,
  menu « Soft Edge (n) » dans la barre AO et case par slice.
- **Tests.** Unitaires : partage d'un recouvrement 900–1100, somme des
  poids, courbe. GPU : deux écrans qui se recouvrent de 4 px, la lumière
  additionnée reste à 100 % ± 8 %.

À vérifier sur de vrais projecteurs : le réglage `gamma` qui compense le
gamma réel des projecteurs.

### 1.3 Mires de test (fait)

Menu « Pattern » dans la barre de l'AO (tous les écrans) et réglage « Test
Pattern » par écran. La mire remplace les slices de l'écran en sortie ; un
avertissement « Test pattern on n screen(s) » reste affiché dans la barre
tant qu'une mire est active, pour ne pas l'oublier pendant un spectacle. Les
mires ne sont pas enregistrées : elles disparaissent à la fermeture.

Dessinées par `pattern.wgsl`, sans texture ni police (chiffres 7 segments) :

- **Grid** : lignes tous les 100 px, cases numérotées, axes centraux blancs,
  bord rouge, cercle inscrit jaune. Géométrie, mise au point, keystone.
- **Colors** : barres blanc, jaune, cyan, vert, magenta, rouge, bleu, noir,
  et gris 50 % en bas. Égaliser les projecteurs (avec `ColorAdjust`).
- **Gradient** : rampe noir → blanc en haut, paliers de 10 % en bas.
  Vérifier le soft edge et le gamma.
- **Slices** : chaque slice dans sa couleur avec son numéro (rang dans
  l'écran) au centre. Ce que la composition envoie où.

Code : `verso_render::Pattern`, `ScreenRenderer::set_patterns`. Test GPU
`test_patterns_replace_the_slices`. L'exemple `ao_screens` prend un 3e
argument (`grid`, `colors`, `gradient`, `slices`, `identify`) pour sortir
les mires d'un preset en PNG.

### 1.4 Identification (fait)

Bouton « Identify » (actif quand l'AO est en Live) : pendant 5 secondes,
chaque écran affiche sa couleur, un cadre de 20 px, son numéro (rang dans la
liste des écrans) et sa résolution. Au branchement, on voit quelle sortie va
sur quel projecteur.

## 2. Cue list (fait le 2026-11-01)

But : une liste ordonnée d'états, rejouée avec un bouton Go et un fondu. Pour
un spectacle rejoué chaque soir, et pour la formation (« cue 3 : le
refrain »). Resolume n'a que les colonnes.

### 2.1 Modèle (`verso-core::cue`)

- Une cue stocke, par couche, la colonne qui joue et l'opacité, plus le
  master et le crossfader ; un nom, un fondu en secondes et une suite
  (`Follow` : attendre Go, après n secondes, après n temps au tempo).
- `CueList` est enregistrée dans le `.verso` ; un fichier sans cues reste
  identique (champ omis quand la liste est vide).
- Le fondu en cours (`Composition::cue_run`) n'est pas enregistré.
- Les cues utilisent des indices : `RemoveLayer` et `RemoveColumn` les
  mettent à jour. À reprendre avec `LayerId` (revue § 3, point 9).

### 2.2 Commandes

- Structure, annulable : `RecordCue` (après la cue courante), `UpdateCue`,
  `RemoveCue`, `MoveCue`.
- Réglages : `RenameCue`, `SetCueFade`, `SetCueFollow`.
- Jeu : `GoCue`, `GoNextCue`, `GoPreviousCue`.

Go lance les clips de la cue avec le fondu comme transition (champ
d'exécution `Layer::cue_transition`, lu par le rendu via
`Layer::transition_time`, effacé au prochain lancement à la main), puis
`Composition::advance_cues`, appelé à chaque image, déplace opacités,
master et crossfader et enchaîne la cue suivante si elle suit seule.
Bouger un fader à la main (opacité de couche, master, crossfader) arrête le
fondu.

Limites assumées : d'une couche vide vers un clip, la transition coupe
(comme pour tout lancement) ; seule l'opacité monte en fondu. Les groupes,
effets et paramètres ne sont pas dans les cues.

### 2.3 Interface

Panneau dockable « Cues » (Window › Cues) : **GO**, Back, Record, Update,
nom de la cue suivante, barre de progression du fondu ; liste avec numéro
(clic = aller à la cue), nom, fondu, suite, monter/descendre, supprimer.
`Espace` = Go partout dans la fenêtre (hors saisie de texte).

### 2.4 Contrôle

- OSC : `/cue/go`, `/cue/previous`, `/cue/N/go`.
- MIDI learn sur GO, Back et le numéro de chaque cue.
- MCP : `apply_commands` accepte les nouvelles commandes ; `get_composition`
  montre la liste, la cue courante et le fondu.

### 2.5 Tests

7 tests dans `verso-core::cue` : enregistrer puis rejouer ; fondu à
mi-parcours et transition des clips ; fader à la main qui arrête le fondu ;
suite en secondes et en temps ; suivante, précédente, insertion,
déplacement, suppression, mise à jour ; suppression de couches et de
colonnes ; ancien fichier sans cues. Les adresses OSC sont dans le test
d'aller-retour de `control`. Essai réel par MCP : Record, Go, Espace.

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
