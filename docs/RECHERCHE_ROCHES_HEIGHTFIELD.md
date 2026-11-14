# Roches et reliefs en heightfield — note technique

Cadre : heightfield `H(x,y)` sur grille régulière (`vf_core::HeightField`), pipeline d'opérateurs déterministes. L'érosion hydraulique par gouttes et le talus thermique existent déjà (`vf_procgen::erosion`, `talus_angle_deg`) : les opérateurs ci-dessous se placent **avant** l'érosion, ou la paramètrent (dureté, caprock).

## 1. Familles, signatures, opérateurs

| Famille | Signature en heightfield | Opérateurs |
|---|---|---|
| Granite | tors, boules, dômes, arêtes émoussées | fBm + Worley cellulaire ; joints orthogonaux ; exfoliation en coques |
| Basalte | orgues, plateaux étagés, dykes | Voronoi 2D extrudé + cross-joints ; terrasses ; crêtes linéaires |
| Calcaire | karst, lapiaz, dolines, falaises | dissolution guidée par fractures ; puits ; rainures ; escarpements |
| Grès | strates, hoodoos, arches, canyons | strates pendées + dureté par couche ; joints verticaux ; caprock |
| Schiste | foliation, crêtes dentelées | bruit étiré anisotrope, ridged orienté, érosion différentielle |
| Gneiss | foliation plissée, yeux | strates + warp sinusoïdal (plis) |
| Conglomérat | galets, relief mamelonné | cellulaire haute fréquence + dureté aléatoire locale |
| Tuf | hoodoos, faible cohésion | érosion rapide, caprock, strates grossières |
| Craie | falaises blanches, pentes convexes | dissolution + érosion hydraulique forte, recul d'escarpement |
| Quartzite | crêtes aiguës, versants raides | ridged multifractal, fractures |
| Glaciaire | cirques, vallées en U, verrous, moraines, roches moutonnées | creusement U, surcreusement, bourrelets, bosses asymétriques |
| Périglaciaire | talus, coulées de blocs, sols polygonaux | talus thermique, seuil de coulée, Voronoi 2D |
| Côtier | falaises, arches, stacks | érosion différentielle + recul ; stacks isolés par joints |
| Désert | yardangs, dunes, pavés | anisotropie de vent, dunes asymétriques, pavage Voronoi |
| Volcanique | cônes, caldeiras, coulées | cône radial + cratère, anneau de caldeira, chenaux de lave |

## 2. Opérateurs et formules

**Strates avec pendage.** Le pendage n'est qu'une rotation du repère :
```rust
let u = x*sin(dip) + z*cos(dip) + warp*fbm(p*0.01);
let t = u / thick;                 // thick = T0*(1.0 + 0.4*noise1d(k))
let k = t.floor(); let f = t.fract();
let bench = smoothstep(0.,0.06,f) * smoothstep(1.,0.94,f);
h += h_k * bench;                  // h_k = hash(k)*contrast
```
Une strate n'est pas une marche : c'est un **banc dur qui ressort après érosion**. Le plus juste est donc de la traiter en dureté (§ suivante), le `bench` ne servant qu'au micro-rebord.

**Érosion différentielle (dureté).**
```rust
let hardness = H(layer(x,z));            // 0..1
let erodibility = mix(1.0, 0.05, hardness);
// dans la boucle goutte : ero_rate_local = ero_rate * erodibility
```
Variante non itérative : `h += (hardness - 0.5) * relief` pour faire affleurer les bancs durs.

**Colonnes prismatiques (basalte).**
```rust
let q = voronoi2(p.xz/spacing + tilt*p.y);   // tilt : évasement des orgues
let joint = smoothstep(0., w, q.f2 - q.f1);  // 0 sur le joint
h -= depth * (1.0 - joint);
```
Cross-joints horizontaux : `if (y/H).fract() < 0.02 { h -= d; }`. Hexagones : grille jitterée, une cellule = un prisme.

**Failles / graben.**
```rust
let s = dot(p.xz, n) - off + 4.*fbm(p*0.02);
h += throw * smoothstep(-w, w, s);
```
Graben = deux failles parallèles de throw opposé ; horst = inverse. `w→0` donne un escarpement net.

**Karst.**
```rust
// dolines : Poisson-disc, cuvette + lèvre
h -= depth * exp(-r*r/(2.*s2)) * (1. - smoothstep(R, 1.3*R, r));
// dissolution guidée par fractures (5 à 20 passes)
let crack = 1. - smoothstep(0., w, q.f2 - q.f1);
h -= dissolve * crack;
// lapiaz : ridged haute fréquence
h += lap * (1. - (noise(p*8. + warp)).abs());
```
Puits : rayon ≥ 1–2 cellules, sinon alias. Un puits vertical franc n'est pas représentable (cf. §4).

**Talus / coulées.** Déjà présent :
```
excess = (hi - hj) - tan(θ)*d
if excess > 0 { transfer = 0.5*excess*relax }
```
Coulée de blocs : seuil `excess > s0`, direction biaisée par bruit, dépôt en lobe.

**Glaciaire.**
```rust
let t = (x.abs()/W).clamp(0.,1.);
h -= depth * (1. - t.powf(p));                  // p≈2.2 → profil en U
h -= over * (-((s/L).powi(2))).exp();           // surcreusement / verrou
h += mor * (-(((s-s0)/Lm).powi(2))).exp() * (-((x/Wm).powi(2))).exp(); // moraine
h += b * (-(x*x/a2 + z*z/c2)).exp() * if z<0. {1.} else {0.35};        // roche moutonnée
```

**Hoodoos / cheminées.** Opérateur émergent, pas direct : caprock + joints isolants.
```rust
let ero = soft * (1. - 0.95*caprock) * (1. + 2.*joint_mask);
```
La colonne survit parce que la couche dure protège sa base : le résultat dépend de l'érosion, il ne se sculpte pas.

**Détail de surface.** Normales d'abord :
```rust
n = normalize(n_geo + k * triplanar_grad(detail_fbm, p, n_geo));
```
Déplacement réel seulement au LOD proche (< 50 m), sinon normales seules.

## 3. Paramètres exposables

| Nom | Plage | Unité | Défaut |
|---|---|---|---|
| `strata_dip` | 0–45 | ° | 8 |
| `strata_thickness` | 0.5–50 | m | 8 |
| `strata_warp` | 0–30 | m | 4 |
| `hardness_contrast` | 0–1 | – | 0.5 |
| `joint_spacing` | 3–200 | m | 40 |
| `joint_width` | 0.05–3 | m | 0.5 |
| `joint_depth` | 0–15 | m | 2 |
| `column_tilt` | 0–20 | ° | 4 |
| `fault_throw` | 0–300 | m | 40 |
| `karst_density` | 0–1 | – | 0.3 |
| `doline_radius` | 5–200 | m | 40 |
| `doline_depth` | 0–60 | m | 10 |
| `lapiaz_amp` | 0–4 | m | 0.4 |
| `talus_angle` | 25–45 | ° | 38 |
| `glacier_depth` | 0–500 | m | 150 |
| `u_power` | 1.5–4 | – | 2.2 |
| `moraine_height` | 0–60 | m | 12 |
| `detail_strength` | 0–2 | – | 1 |

## 4. Limite heightfield et stratégie

Un heightfield est un graphe : **une altitude par (x,y)**. Surplombs, arches, grottes, encorbellements et voûtes sont hors modèle. Une falaise verticale s'étale sur une cellule → effet escalier et normales fausses ; une arche exige un trou dans une paroi, donc une topologie non-graphe.

Approximations crédibles : falaise = pente raide + paroi texturée triplanaire + normal map (lisible au-delà de ~200 m) ; surplomb = « cliff skirt » (bande mesh extrudée le long des crêtes) ; arche/grotte = mesh ou SDF local posé sur le terrain ; stack = aiguille heightfield, acceptable.

Stratégie par étapes, coût/impact :

1. **Normales + triplanar + AO** — aucune géométrie, O(pixels), ~1 jour, fort en vue rapprochée.
2. **Opérateurs heightfield** (strates, joints, karst, glaciaire, talus) — O(N²) par opérateur, quelques ms à 1024², fort sur la silhouette et la lecture géologique.
3. **Falaises verticales** par mesh plaqué sur les crêtes de pente — coût modéré, LOD, fort sur les silhouettes.
4. **Mesh/voxel local** (SDF sparse 64³ + marching cubes) pour arches et grottes « héros » — ×10 à ×100 en mémoire et temps de build, à réserver.
5. **Voxel global** — hors budget CPU ; ne pas voxeliser tout le terrain.

Règle : le heightfield couvre 95 % du terrain, les objets non-graphe sont des meshes locaux.

## Sources

- [GPU Gems 3, ch. 1 — Generating Complex Procedural Terrains](https://developer.nvidia.com/gpugems/gpugems3/part-i-geometry/chapter-1-generating-complex-procedural-terrains-using-gpu)
- [Inigo Quilez — Raymarching terrains](https://iquilezles.org/articles/terrainmarching/)
- [Procedural Landscapes with Overhangs (Musgrave)](https://www.researchgate.net/publication/2948853_Procedural_Landscapes_with_Overhangs)
- [Realistic Simulation of Dissolution Process on Rock Surface (MDPI)](https://www.mdpi.com/1999-4893/17/10/466)
- [Three Ways of Generating Terrain with Erosion Features](https://github.com/dandrino/terrain-erosion-3-ways)
