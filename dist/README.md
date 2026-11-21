# Binaires précompilés

Ce dossier contient les binaires **précompilés** de VistaForge, construits
depuis la source de ce dépôt avec `cargo build --release` :

| Plateforme | Dossier | Compilation |
|---|---|---|
| Linux x86_64 | [`linux-x86_64/`](linux-x86_64/README.md) | Rust 1.99.1, binaires `strip`és |
| Windows x86_64 | [`windows-x86_64/`](windows-x86_64/README.md) | Rust 1.99.1 MSVC |

Chaque dossier contient les trois exécutables et un `SHA256SUMS` :

| Binaire | Rôle |
|---|---|
| `vistaforge` | CLI : génération, import/export DEM et heightmaps, rendu, animation, Blender, MCP tools. |
| `vistaforge-app` | Interface graphique `egui` + viewport GPU wgpu (Vulkan) et aperçu CPU. |
| `vistaforge-mcp` | Serveur MCP stdio (29 outils). |

Tailles Linux : CLI 6,8 Mo, interface 27 Mo, serveur MCP 5,3 Mo, binaires
`strip`és. Tailles Windows (non `strip`és) : CLI 5,4 Mo, interface 21,9 Mo,
serveur MCP 4,2 Mo. Les binaires Linux ont été reconstruits le 20 novembre
2026 et les binaires Windows le 22 novembre 2026 depuis l'arbre de travail
courant (interface GPU/CPU unifiée, tafoni/bancs/joints/oolithes, presets
rocheux enrichis, résolutions 2048–8192, caméra conservée, aperçus
Brouillon → Finale).

## Utilisation

```bash
cd dist/linux-x86_64
./vistaforge --version
./vistaforge --help

# Interface
./vistaforge-app

# Serveur MCP
./vistaforge-mcp --workspace /chemin/vers/projets
```

```powershell
cd dist\windows-x86_64
.\vistaforge.exe --version
.\vistaforge.exe --help

# Interface
.\vistaforge-app.exe

# Serveur MCP
.\vistaforge-mcp.exe --workspace C:\chemin\vers\projets
```

Le viewport GPU utilise wgpu (Vulkan, repli DX12 sous Windows) par défaut.
Sans pilote compatible :

```bash
VISTAFORGE_RENDERER=glow ./vistaforge-app
```

```powershell
$env:VISTAFORGE_RENDERER = "glow"; .\vistaforge-app.exe
```

## Prérequis

- Linux x86_64, glibc ≥ 2.39 (construit sur Ubuntu 24.04, glibc 2.43).
- Windows 10/11 x86_64 (binaires MSVC ; `vcruntime140.dll` requis).
- Pour l'interface : pilote Vulkan (recommandé) ou OpenGL/DX12, X11 ou
  Wayland sous Linux.
- Les fonctions DEM/heightmap ne demandent aucune bibliothèque externe.

## Vérifier l'intégrité

```bash
sha256sum -c SHA256SUMS            # Linux, et Git Bash sous Windows
```

```powershell
Get-FileHash * -Algorithm SHA256   # PowerShell
```

## Notes

- Ces binaires sont produits localement, sans signature de code.
- Les captures, projets `.vfproj` et corpus de recherche ne sont pas inclus.
- Pour macOS, recompiler depuis la source (`cargo build --release -p vf-app
  -p vf-cli -p vf-mcp`).
