# Utiliser VistaForge depuis un autre système (clone + MCP)

Le serveur MCP est un binaire **stdio** : c'est le client MCP (Claude Desktop,
Claude Code, Cursor, VS Code, etc.) qui le lance et parle JSON-RPC 2.0 sur son
entrée/sortie standard. Il ne s'ouvre pas comme une application et n'écoute
aucun port réseau.

## 1. Récupérer le dépôt

```bash
git clone https://github.com/vingaut/vistaforge.git
cd vistaforge
```

### Linux x86_64 : binaire déjà dans le dépôt

```bash
chmod +x dist/linux-x86_64/vistaforge-mcp
./dist/linux-x86_64/vistaforge-mcp --version
# vistaforge-mcp 0.1.0
```

Git conserve le bit exécutable (`100755`) ; si le dépôt a été téléchargé en ZIP,
`chmod +x` est nécessaire.

### Windows x86_64 : binaire déjà dans le dépôt

```powershell
.\dist\windows-x86_64\vistaforge-mcp.exe --version
# vistaforge-mcp 0.2.0
```

### macOS ou autre architecture : recompiler

```bash
# Rust s'installe via https://rustup.rs ; la toolchain 1.99.1 est épinglée.
cargo build --release -p vf-mcp

# Linux/macOS
./target/release/vistaforge-mcp --version

# Windows (PowerShell)
.\target\release\vistaforge-mcp.exe --version
```

Le MCP ne demande **ni GPU ni session graphique**. Pour aussi compiler le CLI et
l'interface :

```bash
cargo build --release -p vf-cli -p vf-mcp -p vf-app
```

## 2. Préparer un workspace dédié

Tous les chemins passés au MCP sont **relatifs au workspace** et confinés :
les chemins absolus et `..` sont refusés. Créez donc un dossier de travail :

```bash
mkdir -p ~/VistaForge
```

Placez-y vos `.vfproj`, ou laissez l'agent les créer avec `project.new`.

## 3. Configurer le client MCP

Remplacez `/CHEMIN/ABSOLU/vistaforge` et `/CHEMIN/ABSOLU/VistaForge` par les
chemins réels. **Utilisez des chemins absolus** : le client lance le serveur
depuis un répertoire qui n'est pas forcément le vôtre.

### Configuration générique (`mcpServers`)

La plupart des clients utilisent ce format :

```json
{
  "mcpServers": {
    "vistaforge": {
      "command": "/CHEMIN/ABSOLU/vistaforge/dist/linux-x86_64/vistaforge-mcp",
      "args": ["--workspace", "/CHEMIN/ABSOLU/VistaForge"]
    }
  }
}
```

Sous Windows :

```json
{
  "mcpServers": {
    "vistaforge": {
      "command": "C:\\CHEMIN\\vistaforge\\target\\release\\vistaforge-mcp.exe",
      "args": ["--workspace", "C:\\CHEMIN\\VistaForge"]
    }
  }
}
```

### Claude Desktop

Fichier de configuration :

- macOS : `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows : `%APPDATA%\Claude\claude_desktop_config.json`
- Linux : `~/.config/Claude/claude_desktop_config.json`

Ajoutez le bloc `mcpServers` ci-dessus, puis redémarrez complètement Claude
Desktop (quitter l'application, pas seulement fermer la fenêtre).

### Claude Code

```bash
claude mcp add vistaforge -- \
  /CHEMIN/ABSOLU/vistaforge/dist/linux-x86_64/vistaforge-mcp \
  --workspace /CHEMIN/ABSOLU/VistaForge
```

Ou, dans un projet, un fichier `.mcp.json` :

```json
{
  "mcpServers": {
    "vistaforge": {
      "command": "/CHEMIN/ABSOLU/vistaforge/dist/linux-x86_64/vistaforge-mcp",
      "args": ["--workspace", "/CHEMIN/ABSOLU/VistaForge"]
    }
  }
}
```

### Cursor / VS Code (clients compatibles MCP)

- Cursor : `~/.cursor/mcp.json` (global) ou `.cursor/mcp.json` (projet).
- VS Code : selon l'extension MCP utilisée, même bloc `mcpServers` avec
  `command` et `args`.

### Vérifier avec MCP Inspector

```bash
npx -y @modelcontextprotocol/inspector \
  /CHEMIN/ABSOLU/vistaforge/dist/linux-x86_64/vistaforge-mcp \
  --workspace /CHEMIN/ABSOLU/VistaForge
```

L'inspecteur affiche l'initialisation, les **29 outils** et permet d'appeler
`project.list` ou `render.preview` à la main.

## 4. Premier échange type

Une fois le serveur connecté, demandez simplement à l'agent :

> Liste mes projets VistaForge, crée `alpes.vfproj` en mode modern preset
> alpine, taille 1026, seed 42, avec érosion, puis rends un aperçu 960×540.

L'agent appellera :

1. `project.list`
2. `project.new` (`name`, `path`, `mode`, `preset`, `size`, `seed`, `erosion`)
3. `terrain.generate` si vous voulez régénérer avec d'autres réglages
4. `render.preview` (`width`, `height`) — l'image est renvoyée dans la réponse
   MCP, pas seulement écrite sur disque
5. `project.save` / `project.info`

Outils principaux :

| Domaine | Outils |
|---|---|
| Projet | `project.new`, `project.open`, `project.save`, `project.info`, `project.list` |
| Terrain | `terrain.generate`, `terrain.import_dem`, `terrain.resample`, `terrain.smooth`, `terrain.height_query`, `terrain.export_dem`, `terrain.export_ascii` |
| Géologie / modificateurs | `geology.set`, `modifiers.kinds/list/add/set/remove/clear/bake` |
| Scène | `scene.preset`, `camera.set`, `sun.set`, `water.set`, `atmosphere.set` |
| Rendu / vol | `render.still`, `render.preview`, `flight.info`, `flight.still` |

## 5. Dépannage

| Symptôme | Cause / remède |
|---|---|
| Le client ne voit aucun outil | Le serveur n'est pas lancé : vérifier `command` et `args`, et que le binaire est exécutable. |
| `Permission denied` | `chmod +x dist/linux-x86_64/vistaforge-mcp` (surtout après un ZIP). |
| Le serveur semble « bloqué » dans un terminal | Normal : il attend du JSON-RPC sur stdin. Laissez le client MCP le lancer. |
| `path escapes workspace` | Utiliser un chemin relatif au workspace ; les chemins absolus/`..` sont interdits. |
| `project.list` est vide | Le workspace ne contient pas de `.vfproj` ; appeler `project.new`. |
| Erreur de rendu GPU | Le MCP n'utilise pas le GPU ; si le CLI/UI échoue, le MCP doit quand même fonctionner. |
| Binaire Linux incompatible | Le binaire précompilé est x86_64/glibc ≥ 2.39 ; recompiler avec `cargo build --release -p vf-mcp`. |

## 6. Sécurité

- Transport **stdio uniquement**, aucun port ouvert, aucun accès réseau.
- Tous les fichiers sont confinés au `--workspace` fourni.
- Les logs vont sur **stderr** ; stdout est réservé au protocole JSON-RPC.
- Utilisez un dossier de travail dédié : l'agent peut créer, modifier et
  supprimer les `.vfproj` de ce workspace.
