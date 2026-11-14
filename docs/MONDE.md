# Grands mondes, tuilage et modes de rendu

Trois échelles, trois outils.

## 1. Terrain unique

Le heightfield stocké va de **2 à 16 384 échantillons par axe**. Le défaut reste
raisonnable (1 026). Au-delà de 8 192, le CLI affiche une estimation mémoire :
16 384² en f32 demande environ **1 GiB** avant les masques, le maillage et les
caches.

```bash
cargo run --release -p vf-cli -- terrain generate --mode modern --size 4096 \
  --preset alpine --seed 42 --name "Grand" --out grand.vfproj
```

Le renderer limite le maillage à 6 M de sommets. Pour les grilles de plus de
2 M d'échantillons, il construit un **maillage adaptatif** : blocs de 64
cellules, niveau de subdivision selon la taille projetée, jupes verticales aux
transitions, budget de sommets. En dessous de 2 M, la grille uniforme est plus
rapide. `--no-adaptive` force la grille uniforme.

## 2. Monde tuilé

`WorldSettings` compose la tuile stockée en N×M instances. Les transformations
(flips, rotations sur tuiles carrées), l'échelle et un décalage vertical
permettent de casser la répétition. C'est le mode **rapide** : les bords ne sont
pas corrigés, donc une couture peut apparaître si la tuile n'est pas déjà
périodique.

```bash
# Configurer seulement
cargo run --release -p vf-cli -- terrain world ile.vfproj --mode tiled \
  --tiles-x 3 --tiles-y 3

# Composer et écrire le heightfield composé
cargo run --release -p vf-cli -- terrain world ile.vfproj --mode tiled \
  --tiles-x 3 --tiles-y 3 --apply --out ile_3x3.vfproj
```

## 3. Monde seamless

Le mode **seamless** corrige d'abord les quatre bords de la tuile, puis
l'assemble. La correction est une rampe localisée dans une bande
(`blend_cells`) : les hauteurs des bords opposés deviennent égales, l'intérieur
reste intact. Les rotations sont désactivées dans ce mode (elles échangent les
bords et casseraient la périodicité) ; les flips restent sûrs.

```bash
cargo run --release -p vf-cli -- terrain world ile.vfproj --mode seamless \
  --tiles-x 3 --tiles-y 3 --blend 48 --macro 55 --macro-scale 1800 \
  --apply --out ile_seamless.vfproj
```

- `--blend` : largeur de la bande de raccord, en échantillons.
- `--macro` / `--macro-scale` : variation de hauteur à l'échelle du monde,
  échantillonnée en coordonnées monde. Elle est continue aux frontières et
  casse l'effet « même montagne répétée neuf fois ».
- `--no-variation` : désactive flips/rotations.

### Ce que « seamless » garantit

- les hauteurs des bords opposés sont **exactes** à 0,01 m près ;
- les colonnes partagées entre deux instances sont identiques ;
- les dérivées ne sont pas corrigées : une arête peut rester visible sur une
  lumière rasante. C'est une correction C0, pas C1.

### Ce que « seamless » ne fait pas

- Une tuile qui est une **île** (bord à la mer) reste une île : le mode
  reproduit le motif, il ne fabrique pas un continent. Générez la tuile avec
  `--border-falloff 0` pour un bord plat et raccordable.
- Pas encore de jeu de tuiles de Wang avec plusieurs variantes : la variation
  vient des flips, des rotations (hors seamless) et du champ macro.

## 4. Modes de rendu

| Mode | SSAA | Ombres | AO | Subdivision | Usage |
|---|---:|---|---:|---:|---|
| `fast` | 1 | non | non | 1 | aperçu, brouillon |
| `refined` | 2 | 4096 | oui | 2 | rendu de travail |
| `photoreal` | 3 | 8192 | oui | 3 | rendu CPU maximal |

```bash
cargo run --release -p vf-cli -- render ile.vfproj hero.png --mode photoreal
```

`photoreal` reste un **rasteriseur déterministe**. Le vrai chemin photoréaliste
est Blender/Cycles (`blender render`), qui utilise les GPU et un dénioseur.
Le DOF est calculé par le rig, écrit dans le sidecar Blender et appliqué par
Cycles ; le rasteriseur CPU ne l'applique pas encore.

## 5. Contraintes de paroi du rig

Le rig construit un **champ de distance aux parois** (cellules de pente > 45°
ou marches d'une cellule). En avant du point de vue, si la distance descend
sous `wall_distance_m`, une accélération latérale proportionnelle pousse la
trajectoire et la caméra loin de la paroi. Le champ est calculé une fois par
simulation.

```bash
cargo run --release -p vf-cli -- rig preset scene.vfproj drone --path ridge
cargo run --release -p vf-cli -- anim still scene.vfproj wall.png --time 3
```

## 6. Limites

- Seamless C0 seulement (pas de correction de pente).
- Pas de tuiles de Wang ni de mélange de plusieurs tuiles sources.
- Le mode tuilé peut montrer des coutures ; le mode seamless les corrige au
  prix d'une bande de raccord.
- Re-composer un projet déjà composé tuile le monde entier : régénérez ou
  rechargez la tuile source avant.
