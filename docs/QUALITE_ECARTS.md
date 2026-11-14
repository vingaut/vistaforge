# Écart de qualité — analyse et plan technique

> Évaluation au **14 novembre 2026**, après le chantier « falaises, lumière et
> modificateurs ». Ce document sert à choisir le prochain chantier, pas à
> décrire l'existant (voir `ETAT_ET_REPRISE.md` et `MODIFICATEURS.md`).

---

## 1. Les trois références, décomposées

| Référence | Ce qui fait le rendu | Ce qu'un rasteriseur CPU doit fournir |
|---|---|---|
| **Canyon** (Grand Canyon) | strates horizontales, corniches, éboulis clairs, rivière, lumière rasante, ciel bleu profond | parois verticales, matériau stratifié, éboulis, eau, contraste |
| **Désert au crépuscule** (Wadi Rum) | silhouettes de mesa, cannelures verticales, poussière en couches, ciel dégradé + étoiles, avant-plan sableux | silhouettes nettes, brume volumétrique, ciel multi-stop, sol de premier plan |
| **Patagonie** (Torres del Paine) | tours de granit verticales, glacier, lac turquoise, éboulis, neige en plaques | **parois verticales**, glace, eau colorée, éboulis |

Le point commun des trois : **des silhouettes verticales**. C'est précisément ce
qu'un heightfield ne peut pas produire nativement.

---

## 2. État des rendus

| Critère | Canyon | Crépuscule | Patagonie |
|---|---|---|---|
| Silhouette | **parois verticales** (pass `cliffs`) | mesas lisibles | **tours verticales** |
| Strates | présentes, lisibles | présentes | absentes (granit) |
| Détail de proche | grain fin, normales atténuées sur les faces | grain fin | grain fin |
| Éboulis | couleur + modificateur `talus` disponible | couleur | couleur |
| Lumière | AO multi-échelle, ombres 4096, rebond | bonne ambiance | meilleure qu'avant, encore plate |
| Ciel | bleu simple | **bon** (bande + étoiles) | bleu simple |
| Eau | désactivée | désactivée | turquoise, reflet trop large |
| Avant-plan | vide | plaine brumeuse | plage étroite |

---

## 3. Diagnostic

1. **La silhouette verticale est acquise, localement.** Le pass `cliffs`
   façonne le heightfield en gradins verticaux sur les faces raides ; le
   canyon et les tours de Patagonie lisent maintenant comme des falaises. Il
   reste deux limites : la crête peut garder une dentelure sur un relief très
   accidenté, et le heightfield interdit toujours surplombs et arches.
2. **La lumière est meilleure mais pas résolue.** L'AO à deux échelles, la
   courbe HBAO, le rebond de sol et les ombres 4096 donnent du modelé ; les
   faces à l'ombre restent sombres et rouges dans un canyon profond, et les
   scènes alpines restent un peu plates.
3. **Les matériaux sont toujours des couleurs.** Les strates et les veines se
   lisent mieux sur les faces verticales, mais le détail métrique reste
   invisible à distance : il n'y a pas de texture de paroi projetée.
4. **L'avant-plan n'existe pas.** Pas de rivière, pas de sable, pas de blocs :
   les cadres bas montrent une plaine brumeuse.

---

## 4. Plan technique et état

### 4.1 Falaises (fait, avec une déviation assumée)

Le plan initial proposait un **mesh local séparé** : une bande verticale de
deux rangées de sommets plaquée sur la crête, avec un nouveau `KIND_CLIFF`.
Deux implémentations ont été essayées :

- un rideau par cellule de pied de falaise : aiguilles et rayures détachées,
  impossible à raccorder au maillage ;
- un rideau le long des arêtes de la frontière face/plat : plus continu, mais
  toujours des lames verticales isolées et des trous de ciel.

La version retenue façonne le **heightfield lui-même** (dans le style de
HighMap/`recast_cliff`, mais avec un ladder rapide) : sur les pentes > 48° et
qui se détachent de leur moyenne locale à 120 m, les altitudes sont aimantées
sur une échelle de 18 cellules (80–250 m) et seules les cellules descendantes
sont modifiées. Le poids est lissé sur quelques cellules pour éviter les
pics d'une cellule. Le résultat :

- murs verticaux continus, qui projettent et reçoivent les ombres ;
- matériau rocheux normal, strates et veines comprises ;
- pas de z-fighting, pas de géométrie flottante ;
- option `cliffs` (activée par défaut), `--cliffs/--no-cliffs`, case dans l'UI,
  `cliffs` dans le schéma MCP.

Le modificateur `recast_cliff` reprend l'algorithme Poisson de Hesiod/HighMap :
il donne des murs plus lisses (sans gradins) au prix de plusieurs secondes de
solveur. Il est disponible pour les plans héros.

Point restant : une **dentelure de crête** peut subsister sur un relief très
découpé, car les cellules plates du bord restent à leur altitude d'origine
pendant que la paroi descend. Un lissage de crête plus large, ou le recast,
l'atténue.

### 4.2 Lumière (fait)

- **AO multi-échelle** : un balayage large (12 % de l'étendue) et un balayage
  fin (6 cellules, 12 directions), combinés.
- **Courbe HBAO** : `gain(ao, 3)` puis `pow(ao, 0.5)` sur la couleur finale,
  comme le shader de Hesiod.
- **Ombres 4096** en qualité High (2048 auparavant), PCF 5×5 à poids radiaux,
  biais dépendant de `N·L`.
- **Rebond** : ciel d'horizon plus présent sur les faces verticales, rebond
  chaud du sol dans le canyon.
- **Normales de détail** atténuées et élargies sur les faces raides : le
  corduroy des marches a disparu.

### 4.3 Matériau de paroi (partiel)

Les strates colorées suivent les corniches, les veines sont limitées aux faces,
et les normales de détail sont atténuées sur les faces. Il manque encore une
**texture procédurale de paroi** (strates + fissures + patine projetées sur le
plan vertical) et un grain visible à 500 m. Le modificateur `stratify` et le
preset `canyon` fournissent une base.

### 4.4 Avant-plan (non fait)

- Rivière : polyline de talweg + ruban d'eau, ou simple lac.
- Sable : matériau + ondulations (`dunes` peut servir).
- Blocs : quelques dizaines de rochers instanciés au pied des falaises.

Le modificateur `talus` construit désormais l'**éboulis géométrique**, ce qui
règle une partie du problème au pied des parois.

---

## 5. Essais déjà faits (à ne pas refaire)

| Essai | Résultat |
|---|---|
| Strates par déplacement vertical | visible, mais doux ; il faut des corniches |
| Tours par cônes (`spires`) | pointes, pas des parois ; inadapté au heightfield |
| Détail de normale en XY | glisse sur les faces → corrigé en repère tangent |
| Grain d'albédo métrique | invisible à 700 m, utile en gros plan |
| Veines de joints partout | grillage sur les pentes → restreint aux faces raides |
| Ciel à dégradé `pow(0.35)` | ciel entier ambré au crépuscule → bande d'horizon nette |
| Plan de sol 30× terrain | bande laiteuse → réduit à 8× |
| Reflet solaire ×4 | tache blanche → réduit à 1,3 |
| Rideau vertical par cellule de pied | aiguilles et rayures détachées → abandonné |
| Rideau le long des arêtes face/plat | plus continu, mais lames isolées et ciel visible entre elles → abandonné |
| Ladder de falaises sur toutes les pentes | rizières sur les cônes (Patagonie) → masque de relief local + seuil 48° |
| Ladder avec poids par cellule | pics de crête d'une cellule → poids lissé + clamp d'enveloppe locale |
| Recast Poisson à 200 itérations | murs raides mais bruit de solveur (moiré) → conservé comme modificateur, pas comme pass par défaut |
| Recast Poisson à 400 itérations + lissage de la déformation | crête lisse mais paroi trop douce → pass par défaut = ladder rapide |
| Médiane/ouverture 3×3 de la crête | sans effet visible sur les dentelures → supprimé |

---

## 6. Critère de sortie

On pourra dire que l'objectif est atteint quand, sur les trois scènes de
démonstration 2050² :

1. au moins une silhouette verticale crédible est présente ;
2. les parois ont un matériau stratifié/fissuré lisible à 500 m ;
3. l'avant-plan n'est plus vide ;
4. les tests et clippy restent verts.

**État au 14 novembre :**

1. **Atteint** : canyon (`demo-out/qual/canyon_hero3.png`) et tours de
   Patagonie (`patagonia_hero3.png`).
2. **Partiel** : les strates sont lisibles, mais le matériau reste coloré ;
   il manque une texture de paroi.
3. **Non atteint** : l'avant-plan est toujours vide (l'éboulis est un début).
4. **Atteint** : `cargo test --workspace` et
   `cargo clippy --workspace --all-targets -- -D warnings` passent.

---

## 7. Ce que Hesiod / HighMap a apporté

Lecture de <https://github.com/ottolink-dev/Hesiod> et de son noyau
<https://github.com/ottolink-dev/HighMap> (GPLv3). Les implémentations Rust
sont indépendantes, aucune ligne GPL n'est copiée ; seules les idées
mathématiques sont reprises.

| Idée | Source | État dans VistaForge |
|---|---|---|
| HBAO à gain et AO sur la couleur finale | shader lit pass | appliqué |
| PCF 5×5 radial + biais `N·L` | shader lit pass | appliqué |
| Strates gamma + bruit par banc | `strata_terrace.cl` | appliqué (`terrace`) |
| Dune asymétrique | `WaveDuneFunction` | appliqué (`dunes`) |
| Remplissage d'éboulis par tas | `fill_talus.cpp` | appliqué (`talus`) |
| Recast cliff Poisson/SOR | `recast_cliff.cpp` | appliqué (`recast_cliff`) |
| Brouillard volumétrique intégré en hauteur | shader lit pass | à faire |
| Scattering Rayleigh/Mie 32 pas | shader lit pass | à faire |
| Neige par champ de profondeur + avalanche | `snow_simulation.cpp` | à faire |
| Vagues de Gabor + écume sigmoïde | shader eau | à faire |
| Ombres cuites `shadow_heightmap` | `shadows.cpp` | piste pour l'aperçu |

Voir [`MODIFICATEURS.md`](MODIFICATEURS.md) pour les 21 opérateurs et leurs
paramètres.

---

## 8. Prochaine session

1. **Neige / glacier** : porter le principe du champ de profondeur + avalanche
   (talus de neige), au lieu de l'accumulation locale actuelle.
2. **Avant-plan** : rivière ou lac, sable, quelques blocs instanciés.
3. **Brouillard volumétrique** : intégrale analytique en hauteur (peu coûteuse)
   puis scattering si le budget le permet.
4. **Texture de paroi** : strates/fissures projetées en repère tangent, grain
   visible à 500 m.
5. **Crête de falaise** : lissage large ou recast par défaut en qualité Ultra.
6. **Eau** : vagues de Gabor, écume sigmoïde, transparence par profondeur.
