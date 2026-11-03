# Serveur MCP intégré

Date : 2026-11-01. Statut : implémenté (crate `verso-mcp`, module
`crates/verso-app/src/app/mcp.rs`).

But : laisser un agent (Claude Code) inspecter et piloter Verso sans captures
d'écran ni clics. Lire l'état, envoyer des commandes, recevoir des images
rendues par le GPU.

## 1. Transport

**HTTP streamable (MCP 2025-06-18), sur `127.0.0.1` uniquement, dans le
processus Verso.** Pas de binaire relais.

- Claude Code parle ce transport nativement :
  `claude mcp add --transport http verso http://127.0.0.1:7400/mcp`.
- Le serveur répond à chaque `POST /mcp` par un seul objet JSON
  (`Content-Type: application/json`), ce que le transport autorise. Pas de
  flux SSE, pas de notification serveur → client, donc `GET /mcp` renvoie 405.
  Pas d'identifiant de session : le serveur est sans état.
- Serveur HTTP/1.1 minimal sur `std::net` (un thread par connexion, keep-alive),
  sans tokio : quelques centaines de lignes, aucune dépendance lourde.
- Le thread HTTP ne touche jamais la composition. `initialize`, `ping` et
  `tools/list` sont servis sur place ; `tools/call` passe par un canal vers la
  boucle egui, qui traite l'appel au début de la frame suivante et répond.
  Délai : une frame (≈16 ms), plus les frames demandées pour une capture.

Un stdio relais aurait demandé un second binaire et un IPC de toute façon ; le
transport HTTP suffit et se teste avec `curl`.

## 2. Activation et instances multiples

Désactivé par défaut. On l'active au lancement :

- `verso --mcp` (port 7400) ou `verso --mcp 7411` ;
- ou la variable `VERSO_MCP_PORT=7411`.

Activer le serveur au lancement règle le problème des instances multiples :
l'instance que Claude lance avec `--mcp` est celle qu'il pilote, même si
l'utilisatrice a sa propre fenêtre `verso.exe` ouverte. Si le port est pris,
Verso démarre quand même et l'écrit dans le journal et la barre d'état. L'outil
`status` renvoie le PID, le port et le chemin du journal pour vérifier qu'on
parle à la bonne instance.

Une instance lancée avec `--mcp` écrit son journal dans `verso-mcp-<port>.log`
(dossier utilisateur), pas dans `verso.log` : une instance de test n'écrase plus
le journal de l'instance en service. L'autosave d'une composition sans fichier
(`Untitled.verso.bak`) reste partagé.

Les appels MCP envoient `ViewportCommand::RequestPaintWhileHidden` : l'UI
continue de tourner fenêtre minimisée ou cachée, donc captures, entrées
injectées et liste des widgets marchent sans ramener la fenêtre devant.

## 3. Sécurité

- Écoute sur `127.0.0.1` seulement, jamais `0.0.0.0`.
- Protection contre le DNS rebinding : l'en-tête `Host` doit être
  `127.0.0.1:<port>` ou `localhost:<port>`, et un en-tête `Origin` éventuel doit
  désigner `localhost` ou `127.0.0.1`. Sinon 403.
- Les actions destructives demandent `"confirm": true` dans les arguments,
  sinon l'appel échoue avec un message explicite : supprimer une couche, une
  colonne, un groupe ou un script, ouvrir, sauver, nouvelle composition.
  Lancer, éjecter ou tout arrêter (`/clear`) ne sont que de la lecture et
  restent libres, comme la touche Échap. La règle est dans `verso-mcp` (`requires_confirm`) et
  s'applique aussi aux commandes brutes de `apply_commands`.
- Pas d'outil d'exécution arbitraire (pas de Lua via MCP).

## 4. Outils

Les indices des outils suivent l'OSC : **base 1** (`layer: 1` = couche du bas).
Les commandes brutes (`apply_commands`) restent en base 0, comme `Command`.

### Lecture

| Outil | Rôle |
|---|---|
| `status` | PID, version, FPS, nom, fichier, modifié, taille, port, journal, FFmpeg |
| `get_composition` | Couches, colonnes, clips (source résumée), clip actif, effets, groupes, master, cues (liste, cue courante, fondu en cours). `full: true` → la composition sérialisée complète |
| `get_layer` | Propriétés d'une couche et ses effets avec définitions de paramètres et valeurs courantes, adresses OSC |
| `get_clip` | Le clip complet (source, effets, paramètres, adresses OSC) |
| `list_sources` | Générateurs intégrés, shaders générateurs, familles MilkDrop (filtre, limite) |
| `list_effects` | Effets (filtres) avec leurs paramètres |
| `get_log` | Messages récents de l'app (barre d'état) et fin du fichier journal |

### Action (même chemin que l'UI : `Command` → `apply_pending`, donc undo et
« modifié » cohérents)

| Outil | Rôle |
|---|---|
| `osc` | Une adresse de l'espace OSC existant (`/layer/2/opacity 0.5`, `/column/3/launch`, `/layer/1/effect/1/radius`, `/composition/...`). Passe par `Action::parse_osc` comme un message UDP |
| `set_clip` | Pose une source dans une case : générateur, couleur, shader, MilkDrop (famille + variante), fichier image/vidéo, ou vide |
| `set_milkdrop` | Change variante, fondu, entrée d'un clip MilkDrop |
| `add_effect` | Ajoute un effet de la bibliothèque à une chaîne |
| `apply_commands` | Liste de `Command` en JSON (sérialisation serde) pour tout le reste |
| `select` | Sélectionne une couche ou un clip (panneaux de propriétés, moniteur) |
| `hover` | Simule le survol d'un en-tête de couche, de colonne ou d'une case, pour l'aperçu au survol |
| `ui_input` | Vraies entrées souris et clavier dans la fenêtre, via `eframe::App::raw_input_hook` : `move` (survol), `click`, `double_click`, `right_click`, `middle_click`, `drag` (glisser-déposer, 8 pas), `scroll`, `key`, `text`, `leave`, avec modificateurs. Coordonnées en points UI. Un événement par frame, réponse deux frames après le dernier |
| `ui_widgets` | Widgets de la frame : rôle, libellé ou valeur, état, rectangle et centre en points, lus dans l'arbre AccessKit qu'egui construit (activé le temps d'une frame). Plus le rectangle et le nom de chaque case de clip. Les widgets dessinés à la main (cases, éléments du navigateur) n'y ont pas de libellé : les viser par `clip_slots` ou par la capture `ui` |
| `undo`, `redo` | Comme Ctrl+Z / Ctrl+Shift+Z |
| `save`, `open`, `new_composition` | Destructifs : `confirm: true` |

### Vue

`capture` renvoie un PNG (contenu image MCP) produit par Verso, sans capture
d'écran de l'OS :

- `ui` : la fenêtre entière telle qu'egui la peint (`Context::request_screenshot`),
  menus contextuels et fenêtres flottantes compris. La légende donne la taille
  en points UI, l'unité de `ui_input` ;
- `output` : la sortie composée, lue sur le GPU ;
- `source` : une source hors composition (même format que `set_clip`), dans
  une composition jetable ; sources procédurales seulement ;
- `layer`, `column`, `clip` : rendus hors écran par un second `Renderer`, comme
  l'aperçu au survol (couche en solo, colonne lancée, clip lancé et en solo),
  sans toucher la sortie ;
- `hover` : la texture réelle de l'aperçu au survol (après `hover`).

`frames` laisse tourner le rendu avant la lecture (60 par défaut hors écran :
un preset MilkDrop neuf démarre souvent noir et met une à deux secondes à se
remplir), `width` réduit l'image (640 par défaut). Le renderer hors écran
reprend les bibliothèques d'effets et de MilkDrop du renderer principal, sans
relire le disque.

Boucle type pour tester l'UI : `capture target=ui`, repérer la cible
(`ui_widgets`, ou coordonnées lues sur l'image divisées par
`pixels_per_point`), `ui_input`, puis `capture target=ui` ou `get_composition`
pour vérifier.

## 5. Tests

- `verso-mcp` : protocole (initialize, tools/list, notifications, erreurs),
  garde-fous (`Host`, `Origin`, `confirm`), aller-retour `tools/call` via le
  canal, résumé de composition et analyse des sources.
- `verso-core` : sérialisation JSON aller-retour de `Command`.
- Vérification manuelle : Claude Code connecté, appels réels (voir README).
