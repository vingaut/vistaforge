# Recherche — surfaces rocheuses procédurales

> Support de la session du 20 novembre 2026. Objectif : transformer la
> **géométrie** du terrain, pas seulement son albédo, en s'appuyant sur des
> signatures géologiques nommées.

## 1. Familles et signatures

| Famille | Exemple | Signature de surface | Couches procédurales |
|---|---|---|---|
| Sédimentaire | calcaire, grès, craie, travertin | bancs, cross-bedding, tafoni, oolithes | `benches`, `tafoni`, `ooliths`, `grooves` |
| Ignée | granite, basalte, andésite | joints orthogonaux ou columnaires, vésicules, exfoliation | `joints`, `vesicles`, `exfoliation`, `craters` |
| Métamorphique | schiste, phyllite, ardoise, gneiss, marbre | foliation, plis, plans de clivage, bancs fins | `grooves`, `ridges`, `benches`, `joints` |
| Détritique | conglomérat | galets arrondis dans une matrice | `ooliths` (grands rayons), `tafoni` |
| Siliceux | chert, silex | cassure conchoïdale, surface lisse, joints espacés | faible `amplitude`, `joints` larges |

## 2. Algorithmes retenus

### Tafoni / nids d'abeille

Les tafoni sont des alvéoles de météorisation saline, souvent sur des faces
verticales, avec des parois minces. Le modèle procédural :

1. déformer les coordonnées par un fBm basse fréquence ;
2. calculer un champ cellulaire (F1, F2, identifiant de cellule) ;
3. creuser un bol `-depth · (1 - smoothstep(0, 0.72, F1))` ;
4. ajouter un bourrelet de paroi autour de `F1 ≈ 0.5` ;
5. masquer par la couverture, la pente (faces verticales), une tache de
   basse fréquence et l'empreinte pixel.

Références : [Porous space — biomimetic of tafoni in computational
design](https://link.springer.com/article/10.1007/s44223-022-00019-4) ;
[Proc-Rock](https://github.com/acfaruk/proc-rock) pour le mapping familles →
techniques procédurales.

### Bancs et érosion différentielle

Un banc dur forme une saillie, un banc tendre un creux. On calcule une
coordonnée stratigraphique `s = z - tan(pendage)·(x·cos az + y·sin az)`,
on ajoute une variation d'épaisseur, puis un profil en marche
(`smoothstep` sur la phase) pour obtenir un replat et un riser. Le masque de
pente évite les bancs sur les prés.

Référence : [Flexible terrain erosion
(LIRMM)](https://www.lirmm.fr/~nfaraj/publications/flexible_erosion/2024_Flexible_Terrain_Erosion.pdf)
pour le couplage érosion / couches.

### Joints et colonnes

Deux réseaux de droites tournées, avec jitter par cellule ; la distance à la
droite la plus proche est convertie en rainure. Pour le basalte, les deux
familles à 60° approchent les prismes hexagonaux ; les approches Voronoi
donnent des colonnes irrégulières.

Référence : [MDPI — Columnar Jointed Rock
Mass](https://www.mdpi.com/2073-8994/15/4/944) ; [tutoriel Voronoi
WGPU](https://dev.to/the_lone_engineer/tutorial-procedural-voronoi-texture-generation-in-wgpu-1b3k).

### Oolithes / bioclastes

Champ cellulaire dont chaque cellule reçoit une sphère aplatie ; la couverture
et la taille varient par cellule. Sur un heightfield, cela produit des grains
proud du matrix, pas des sphères complètes (pas de surplomb).

### Exfoliation, vésicules, cratères, sillons

Déjà présents : feuillets courbes, puits volcaniques, cratères à rebord,
canaux alignés sur la pente. Les nouveaux profils les combinent avec les
couches ci-dessus.

## 3. Limite heightfield

Un heightfield ne peut pas représenter un surplomb, une arche ou une grotte.
Les tafoni sont donc des **bols ouverts**, les joints des rainures, les bancs
des marches. Pour les arches et les surplombs, il faudra un mesh local ou un
SDF (hors périmètre de cette session).

## 4. Presets livrés

- détail : `granite`, `basalt`, `limestone`, `sandstone`, `schist`, `chalk`,
  `volcanic`, `canyon`, `patagonia`, `chert`, `andesite`, `marble`, `phyllite`,
  `slate`, `gneiss`, `conglomerate`, `travertine` ;
- matière : mêmes familles + `generic`.

Le lien **« Lier la géométrie de détail au preset de roche »** applique le
profil de détail correspondant quand la roche change, sans écraser la tolérance
adaptative ni les quads.
