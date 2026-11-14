# Construire VistaForge

VistaForge est un workspace Rust 2024. La toolchain est épinglée dans
[`rust-toolchain.toml`](../rust-toolchain.toml) (Rust 1.99.1) et s'installe
automatiquement avec rustup.

## Prérequis

### Ubuntu 24.04+

```bash
sudo apt update
sudo apt install -y build-essential curl
# Bibliothèques d'exécution de l'interface (présentes sur un bureau standard) :
sudo apt install -y libxkbcommon0 libwayland-client0 libgl1 libegl1
```

Installer rustup si nécessaire : <https://rustup.rs/>.

### Windows 11

1. Installer [rustup](https://rustup.rs/).
2. Installer les **Visual Studio Build Tools** avec la charge de travail
   « Desktop development with C++ » (éditeur de liens MSVC).
3. Ouvrir un terminal PowerShell dans le dépôt.

La toolchain épinglée est téléchargée automatiquement au premier `cargo`.

## Compiler et tester

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Le corpus legacy est optionnel : les tests qui en dépendent s'ignorent
proprement s'il est absent. Pour les exécuter, placer les 38 `.DE` dans
`re/unpacked/` (voir le [README](../README.md)).

## Lancer

```bash
# CLI
cargo run -p vf-cli -- --help

# Interface de bureau
cargo run -p vf-app

# Serveur MCP (stdio)
cargo run -p vf-mcp -- --workspace .
```

## Version release

```bash
cargo build --release -p vf-cli -p vf-mcp -p vf-app
./target/release/vistaforge --help
./target/release/vistaforge-mcp --workspace .
./target/release/vistaforge-app
```

Sous Windows, les binaires sont dans `target\release\` avec l'extension
`.exe`.

## Architecture matérielle

- **Rendu** : CPU déterministe, multi-thread via `rayon`. Aucun GPU requis.
  Un terrain 1026² en 1920×1080 qualité haute se rend en quelques secondes sur
  un CPU de bureau.
- **Interface** : `eframe`/`egui` avec le backend OpenGL (`glow`). Un pilote
  OpenGL 3.3+ est nécessaire pour la fenêtre ; le CLI et le MCP fonctionnent
  sans affichage.
- **MCP** : transport stdio, JSON-RPC 2.0, aucun accès réseau.

## CI

[`ci.yml`](../.github/workflows/ci.yml) exécute formatage, clippy et tests sur
Ubuntu 24.04 et Windows à chaque push et pull request.

## Dépannage

| Symptôme | Cause probable / remède |
|---|---|
| `failed to initialize any backend` au lancement de l'UI | Pas de session graphique ; utiliser le CLI ou le MCP, ou lancer depuis un bureau. |
| Erreur OpenGL au lancement de l'UI | Pilote OpenGL absent ; installer `libgl1`/`libegl1` ou utiliser le rendu CPU via le CLI. |
| `cargo` télécharge une toolchain | Normal : `rust-toolchain.toml` épingle 1.99.1. |
| Tests corpus ignorés | `re/unpacked/` absent ; comportement attendu. |
| Rendu très lent en debug | Utiliser `--release` : le renderer est optimisé pour la release. |
