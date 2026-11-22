# Viewport 3D GPU (wgpu / Vulkan)

VistaForge conserve un rendu final **CPU déterministe** pour les exports PNG et
le CLI. Le viewport de l'interface de bureau peut en plus être dessiné par le
GPU via `wgpu` : Vulkan sous Linux/Windows, Metal sur macOS, DX12/OpenGL en
repli. Ce document décrit ce chemin, ce qu'il montre et ce qu'il ne montre pas,
puis le second usage du GPU : le **calcul géométrique** des maillages.

## Pourquoi un chemin séparé

Le rasteriseur CPU est la référence : ombres, AO, nuages, détail de roche,
eau avec écume et tone mapping ACES. Il n'est pas transposable tel quel dans un
shader sans réécrire tout le rendu. Le viewport GPU est donc un **aperçu de
navigation** : il doit être fluide pendant l'orbite, le zoom et les réglages,
pas pixel-identique au rendu final.

Le viewport GPU :

- applique la pile de modificateurs (`vf_modifiers`) et le pass falaises
  (`vf_procgen::cliff::sharpen`) avant de construire le maillage ;
- affiche un ciel dégradé avec soleil, un relief éclairé par le soleil, une
  eau opaque avec Fresnel/reflet, et du brouillard ;
- utilise la palette, l'azimut/déclinaison du soleil et l'exposition du projet ;
- respecte l'activation de l'eau (panneau Eau et case Water du panneau Rendu) ;
- ne montre pas les ombres portées, l'AO, les nuages, le détail de roche, la
  subdivision adaptative, les matériaux avancés ni le post-traitement.

Le rendu final (`Render`) reste produit par `vf-render` sur le CPU, à la
résolution de sortie, et devient l'image affichée jusqu'au prochain changement.

## Architecture

```
 UI (egui)                         render thread (eframe/wgpu)
 ─────────                         ───────────────────────────
 heightfield ──► build_terrain ──► vertex/index buffers
 project     ──► uniforms       ──► uniform buffer
 camera      ──► callback.prepare ─► offscreen color + Depth32Float
                                  └► sky pass, terrain pass
 callback.paint ─────────────────► blit texture dans le viewport egui
```

Points clés :

- **Cible offscreen** : le render pass d'egui n'a pas de depth buffer. Le
  callback rend donc le ciel et le terrain dans une texture couleur + depth,
  puis un triangle plein écran échantillonne cette texture dans le pass egui.
- **Callback egui-wgpu** : `GpuCallback` est ajouté au painter pour le rectangle
  du viewport. `prepare` encode le rendu offscreen dans l'encodeur d'egui ;
  `paint` blitte la texture. Aucun command buffer séparé n'est nécessaire.
- **Maillage** : échantillonnage du heightfield avec un pas calculé pour ne pas
  dépasser 2 049 segments par côté (≈ 4,2 M de sommets au maximum). Un terrain
  2050² est donc affiché à sa résolution native ; 4096² est encore réduit de
  moitié. Les normales
  sont calculées par différences centrées. Une grande nappe d'eau est ajoutée
  au maillage lorsque l'eau est active.
- **Coordonnées** : les sommets et la caméra sont translatés par rapport au
  centre du terrain pour rester en `f32` avec une bonne précision.
- **Reconstruction** : le maillage GPU est reconstruit après un debounce de
  150 ms lorsqu'un réglage de scène change. Les changements de caméra seuls ne
  reconstruisent rien : le callback lit la caméra à chaque image.

## Shaders

- `src/shaders/scene.wgsl` : `vs_sky`/`fs_sky` (triangle plein écran + rayons),
  `vs_terrain`/`fs_terrain` (relief, eau, brouillard, exposition).
- `src/shaders/blit.wgsl` : copie de la texture offscreen vers le viewport.

Les couleurs de palette sont linéaires. Si la surface cible n'est pas sRGB, le
shader encode explicitement en sRGB ; sinon le GPU s'en charge à l'écriture.

## Bascule et dépannage

- Par défaut, l'application démarre avec `eframe::Renderer::Wgpu`.
- **Préférences → Calcul (CPU / GPU)** liste les adaptateurs wgpu détectés. Choisir
  un GPU (ou un profil performance/économie) enregistre le réglage ; l'adaptateur
  de l'**aperçu** s'applique au **prochain lancement**. Sur une machine à deux GPU
  (portable, eGPU), cela permet de forcer le discret ou l'intégré.
  `VISTAFORGE_GPU` accepte `auto`, `off` ou un nom d'adaptateur (recherche
  insensible à la casse).
- `VISTAFORGE_RENDERER=glow` force le renderer OpenGL d'egui et le viewport CPU.
- Dans le panneau **Rendu**, la case **Vue 3D GPU** active/désactive le
  viewport GPU. Le libellé indique l'adaptateur et le backend détectés
  (par exemple `NVIDIA … · Vulkan`).
- Le choix de l'adaptateur d'aperçu ne déplace pas les calculs de relief,
  d'érosion ni le rendu final : ils restent sur le CPU, répartis sur les cœurs
  choisis dans la même section des préférences. Le calcul **géométrique** des
  maillages, lui, peut utiliser le GPU : voir la section suivante.
- Si la création du viewport GPU échoue (shader, adaptateur, mémoire), l'erreur
  est affichée dans le panneau Rendu et l'aperçu CPU prend le relais.

## Calcul géométrique CPU / GPU (`vf-compute`)

Le rendu n'est pas le seul travail parallèle de VistaForge. Les transformations
géométriques du maillage 3D — normales de sommet (angle pondéré et aire
pondérée), lissage de Taubin, déplacement le long des normales pour le détail
rocheux, transformation affine — passent par le crate `vf-compute`, qui offre
deux moteurs interchangeables. L'évaluation d'un trait de pinceau, elle, reste
sur le pool CPU (chaque sommet y est indépendant) ; ce sont les recalculs de
normales et de déplacement qui suivent le moteur choisi.

| Moteur | Ce qu'il utilise | Sélection |
| --- | --- | --- |
| `CPU` | le pool `rayon` partagé de `vf_core::compute`, 1 à N threads | **Cœurs de calcul** |
| `GPU` | un adaptateur wgpu (calcul), pipelines WGSL | **Adaptateur** |
| `GPU (tous)` | tous les adaptateurs détectés, plage de sommets découpée entre eux | cases à cocher |

Le choix se fait dans **Préférences → Calcul (CPU / GPU)**, ou directement par le
sélecteur **Calcul** du panneau **Maillage 3D**. Il s'applique **immédiatement** :
`vf_compute::set_backend` vide le cache de périphériques et le noyau suivant
ouvre les adaptateurs demandés. La préférence est persistée dans `compute.ron`.

Points de conception :

- **Déterminisme** : le CPU découpe en blocs fixes (`CHUNK = 4096` sommets) et
  réduit dans un ordre fixe, indépendant du nombre de threads. Les structures de
  voisinage (`FaceGraph`, `VertexFaces`, `VertexCorners`) trient les indices, si
  bien que la somme des normales se fait dans l'ordre de référence.
- **Seuil GPU** : en dessous de 2 048 sommets, le transfert de buffers coûte plus
  cher que le noyau CPU, qui reste utilisé.
- **Repli** : un adaptateur introuvable, un appareil qui ne s'ouvre pas ou un
  noyau WGSL en échec renvoie `None` ; l'appelant exécute le chemin CPU avec le
  même résultat. `vf_compute::last_error()` et la ligne **Moteur actif** des
  préférences expliquent le repli.
- **Multi-GPU** : `run_split` divise la plage `0..len` en tranches contiguës,
  une par périphérique, chacune dans son thread, puis réassemble dans l'ordre.
- **Coût par appel** : l'énumération des adaptateurs est mise en cache (elle
  coûtait ~130 ms par appel sur Vulkan), tous les passes d'un noyau sont encodés
  dans **une seule** soumission, et la copie de retour est ajoutée à la même
  soumission : un appel GPU reste à ~0,25 ms de latence incompressible.
- **`--compute` / `VISTAFORGE_COMPUTE`** : `cpu`, `gpu`, `all` ; `--compute-gpu`
  / `VISTAFORGE_COMPUTE_GPU` choisissent l'adaptateur (nom ou clé
  `backend|nom`). La CLI et l'application les acceptent.

### Finesse perçue : ce qui a été corrigé dans le viewport

Le viewport 3D ne calculait pas le détail comme le rendu final. Trois corrections
rapprochent les deux :

- **un seul plafond de maillage** : activer le détail divisait la résolution du
  terrain par deux (1 025 au lieu de 2 049 segments) ; c'est le même plafond pour
  les deux chemins ;
- **normales sur la grille native** : les différences centrées utilisaient un
  stencil de ±`step` cellules, ce qui lissait toute crête plus étroite que deux
  quads ; elles utilisent ±1 cellule comme le rendu ;
- **empreinte du détail = une demi-cellule** (`0.5 × spacing`), la même valeur que
  `vf_mesh::build_shape_adaptive` : le viewport atténue désormais les mêmes
  longueurs d'onde que le PNG.

Côté sculpture, le canvas 2D se rafraîchit **pendant** le trait (throttlé à
~120 ms) au lieu d'attendre le relâchement du bouton : c'était la cause principale
de l'impression que « le détail ne tient pas ». Le détail continu reste une
propriété du rendu (et du maillage 3D, qui le cuit) : le heightmap stocké garde sa
résolution, et un détail plus fin que deux cellules demande de densifier la
grille (Bouton **Densifier ×2**) ou de sculpter dans le module 3D.

Le calcul GPU n'est pas un chemin déterministe au bit près : les pilotes peuvent
fusionner multiplications et additions. Les tests comparent le GPU au CPU avec
une tolérance de 2 × 10⁻³ sur les normales ; le CPU reste la référence pour les
exports et les tests.

### CPU ou GPU : ce que disent les mesures

Chaque appel GPU transfère les positions, la topologie et le résultat par le bus
PCIe. Sur les noyaux actuels — gourmands en mémoire, pas en calcul — un CPU
multi-cœur est déjà très efficace, et le GPU ne gagne que lorsque les cœurs
laissés au pool sont peu nombreux. Mesures sur un poste 16 cœurs logiques + RTX
5070, maillage de 393 261 sommets / 1 568 000 coins (release) :

| Noyau | CPU 16 threads | CPU 4 threads | CPU 1 thread | GPU (1 adaptateur) |
| --- | --- | --- | --- | --- |
| Normales de sommet | 8,8 ms | 16,2 ms | 47,6 ms | 13,9 ms |
| Lissage de Taubin, 4 passes | 6,7 ms | 6,5 ms | 17,4 ms | 9,7 ms |
| Déplacement le long des normales | 1,8 ms | 1,5 ms | 1,9 ms | 10,3 ms |

Sur un maillage plus petit (32 361 sommets) le CPU reste devant partout :
normales 1,2 ms contre 1,6 ms au GPU. En pratique :

- **gardez le CPU** pour sculpter sur un poste récent : c'est le chemin le plus
  rapide et il est déterministe ;
- **passez au GPU** si vous limitez volontairement le pool (par exemple 4 cœurs
  pour laisser respirer l'interface et la génération), sur une machine à peu de
  cœurs, ou si vous voulez libérer le CPU pendant un remesh ;
- **tous les GPU** n'aide que si le calcul domine le transfert : chaque
  périphérique reçoit sa propre copie de la topologie, donc deux GPU ne doublent
  pas le débit sur les petits maillages.


## Limites connues

- Pas d'ombres, d'AO, de nuages ni de détail de roche dans le viewport GPU.
- Le maillage est reconstruit sur le thread UI (après debounce) : sur un très
  grand terrain ou une pile de modificateurs lourde, la synchronisation peut
  bloquer brièvement l'interface.
- Le viewport GPU n'est pas un chemin d'export : il ne modifie ni le PNG, ni le
  CLI, ni les résultats MCP.
- La lecture d'une séquence de vol complète reste un rendu CPU (CLI
  `anim render`) ; le GPU couvre l'exploration interactive du terrain.
- Côté calcul géométrique, seuls les noyaux listés ci-dessus ont un chemin GPU.
  Le remesh, la fracture, le voxel/SDF, les métaballs et la physique restent sur
  le pool CPU : ce sont des algorithmes à dépendances de voisinage ou à
  changement de topologie, que le découpage par sommet ne couvre pas.

## Pistes

- Pass d'ombres et d'AO GPU pour rapprocher l'aperçu du rendu final.
- Nuages 2.5D et eau transparente dans le shader.
- Reconstruction du maillage sur un worker pour les très grands heightfields.
- Rendu GPU du détail de roche (texture de détail ou compute).
- Noyaux GPU pour le remesh (lissage de Taubin déjà porté) et la projection sur
  la surface de référence.
- Garder la topologie résidente sur le périphérique entre deux appels (clé de
  révision du maillage) : c'est ce qui manque pour que le GPU dépasse vraiment le
  CPU sur les gros maillages, le transfert par appel étant aujourd'hui le coût
  dominant.
