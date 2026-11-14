# Rig caméra et animation

VistaForge distingue trois objets, comme un rig LightWave :

1. **Point de vue (Eye)** — une courbe 3D à clés temporelles et poignées
   Bézier. C'est l'objet que l'on dessine.
2. **Cible (Target)** — point fixe, courbe propre, ou poursuite du point de vue
   avec une avance en secondes.
3. **Caméra (Optics)** — liée rigidement au point de vue ou suspendue sur un
   ressort, avec FOV, profondeur de champ, roulis explicite et offset.

La dynamique du **corps** (drone, oiseau, hélicoptère, dirigeable, chasseur,
manuel) limite l'accélération de chaque axe et produit le roulis en virage.
Le tout est déterministe : à seed et temps égaux, la caméra est identique.

![Rig hélicoptère](screenshots/rig_helicopter.png)

## Presets de dynamique

| Preset | Vitesse max | Accél. avant | Accél. latérale | Accél. verticale | Roulis max | Signature |
|---|---:|---:|---:|---:|---:|---|
| `drone` | 30 m/s | 25 m/s² | 30 m/s² | 20 m/s² | 75° | nerveux, jerk 60 |
| `bird` | 18 | 8 | 12 | 10 | 55° | inertie moyenne, turbulence |
| `helicopter` | 22 | 4 | 6 | 5 | 25° | lourd, nacelle stable |
| `airship` | 12 | 1,5 | 1,5 | 1 | 8° | très lent, dérive |
| `fighter` | 90 | 40 | 60 | 30 | 85° | haute énergie, jerk 140 |
| `manual` | 200 | 200 | 200 | 200 | 0° | suit la courbe sans ressort |

Le corps suit la courbe par un **ressort critique** ; l'accélération demandée
est projetée sur les axes avant / latéral / vertical, bornée par le preset,
puis limitée en jerk. L'écart entre la courbe et le corps est le « lag »
naturel du drone ou du dirigeable.

## Roulis

Le roulis automatique vaut `atan(a_lat / g) × bank_gain`, borné par
`max_bank_deg` et lissé par `roll_response`. Le roulis explicite de l'optique
s'ajoute. Un preset `manual` conserve la courbe et n'ajoute pas de gîte.

## Contraintes

- **Hauteur au sol** : le point de vue, la caméra et/ou la cible sont remontés
  au-dessus du terrain + `ground_clearance_m`, avec une douceur réglable.
- **Sol de référence fixe** : un plan d'altitude `reference_z_m` peut remplacer
  ou compléter le terrain (utile pour un survol au-dessus d'une mer).
- **Parois** : les champs `wall_distance_m` et `wall_avoidance` sont présents
  dans le modèle ; l'évitement latéral est branché au lot P5.

## Édition graphique

L'espace **Animation** de l'interface affiche le terrain à plat, la courbe du
point de vue en orange, la courbe de cible en bleu et les clés. On peut :

- glisser une clé pour changer X/Y (l'altitude se règle dans le panneau) ;
- ajouter une clé au temps courant ;
- supprimer une clé (minimum deux) ;
- choisir un preset et générer une trajectoire orbite / crête / piqué ;
- régler le FOV, la profondeur de champ, le roulis, la liaison caméra ;
- régler chaque accélération, le jerk, le gain et le roulis max ;
- rendre un aperçu au temps courant.

## CLI

```bash
# Appliquer un preset et une trajectoire orbitale
cargo run --release -p vf-cli -- rig preset ile.vfproj helicopter --path orbit

# Ajouter une clé
cargo run --release -p vf-cli -- rig key ile.vfproj --time 4 --x 5000 --y 5000 --z 1200

# Inspecter
cargo run --release -p vf-cli -- rig info ile.vfproj

# Rendu d'une image du rig (le projet doit avoir rig.enabled)
cargo run --release -p vf-cli -- anim still ile.vfproj plan.png --time 2.4

# Séquence
cargo run --release -p vf-cli -- anim render ile.vfproj frames/ --seconds 8 --fps 24
```

`anim` utilise le rig s'il est activé, sinon l'ancien module de vol
(`flight`). La séquence rig est rendue image par image avec la même simulation
déterministe ; le flou de mouvement du module `flight` n'est pas encore branché
sur le rig.

## Limites

- Pas de flou de mouvement sur le rig pour l'instant.
- L'évitement de parois est modélisé mais pas encore actif.
- Le DOF est calculé et exporté dans `RigSample`, mais le rasteriseur CPU ne
  l'applique pas encore ; le bridge Blender lira ces valeurs au lot P5.
- Pas d'IK, pas d'expressions, pas de graphe de scène complet : le trio
  eye/target/optics couvre le cas cinématique visé.
