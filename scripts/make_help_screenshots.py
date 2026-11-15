#!/usr/bin/env python3
"""Compose the animation help illustrations from real application captures.

Run `VISTAFORGE_CAPTURE=/tmp/vf-captures VISTAFORGE_RENDERER=glow cargo run -p vf-app -- <project>`
first, then this script. It overwrites the generic versions produced by
`make_help_images.py` for the topics that benefit from a real screenshot.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

from make_help_images import (  # noqa: E402
    ACCENT,
    ORANGE,
    arrow,
    canvas,
    font,
    label,
    paste_cover,
    save,
)

CAPTURES = Path(os.environ.get("VISTAFORGE_CAPTURES", "/tmp/vf-captures"))
# The capture window is 1440x900; the viewport starts after the 360 px panel.
VIEWPORT = tuple(
    int(value)
    for value in os.environ.get("VISTAFORGE_VIEWPORT", "360,172,1440,700").split(",")
)


def capture(name: str) -> Image.Image | None:
    path = CAPTURES / f"{name}.png"
    return Image.open(path).convert("RGB") if path.exists() else None


def viewport(name: str) -> Image.Image | None:
    image = capture(name)
    if image is None:
        return None
    return image.crop(VIEWPORT)


def path_image():
    image = viewport("animation_path")
    if image is None:
        return None
    base = canvas("Tracé de l'œil et point de vue", "gizmo de translation sur la clé sélectionnée")
    paste_cover(base, image, (24, 92, 936, 516))
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (40, 106), "E1 sélectionnée", size=22, color=(205, 240, 255))
    arrow(draw, (250, 165), (420, 265), ACCENT, 5)
    label(draw, (600, 106), "gizmo X / Y / Z", size=22, color=(210, 240, 255))
    arrow(draw, (680, 165), (575, 255), ACCENT, 5)
    label(draw, (640, 300), "cible du point de vue", size=22, color=(255, 220, 170))
    arrow(draw, (700, 355), (560, 385), ORANGE, 5)
    label(draw, (40, 440), "la courbe se déforme sur l'axe tiré", size=19, color=(180, 255, 210))
    return base


def target_image():
    image = viewport("animation_target_clean")
    if image is None:
        return None
    base = canvas("Ce que la caméra regarde", "la cible n'apparaît qu'en vue design")
    paste_cover(base, image, (24, 92, 936, 516))
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (46, 110), "œil + tracé cyan", size=22, color=(205, 240, 255))
    arrow(draw, (200, 172), (378, 360), ACCENT, 5)
    label(draw, (600, 130), "cible orange", size=22, color=(255, 220, 170))
    arrow(draw, (660, 190), (500, 385), ORANGE, 5)
    label(draw, (500, 440), "viseur et ligne de visée", size=20, color=(255, 230, 180))
    arrow(draw, (650, 430), (540, 395), ACCENT, 5)
    return base


def profile_image():
    image = viewport("animation_profile_clean")
    if image is None:
        return None
    base = canvas("Même tracé, cinq corps", "les trajectoires divergent selon le profil")
    paste_cover(base, image, (24, 92, 936, 516))
    draw = ImageDraw.Draw(base, "RGBA")
    # Legend panel over the empty sky, away from the trajectories.
    panel = (590, 104, 920, 280)
    draw.rounded_rectangle(panel, radius=10, fill=(8, 12, 17, 220), outline=(70, 94, 120, 230))
    rows = [
        ("Drone FPV (actif, jaune)", (255, 226, 120)),
        ("Oiseau", (120, 205, 255)),
        ("Hélicoptère", (134, 224, 154)),
        ("Ballon dirigeable", (210, 165, 255)),
        ("Avion de chasse", (255, 92, 120)),
    ]
    for i, (text, color) in enumerate(rows):
        y = panel[1] + 16 + i * 31
        draw.line([(panel[0] + 16, y + 9), (panel[0] + 46, y + 9)], fill=color, width=5)
        draw.text((panel[0] + 58, y - 2), text, font=font(19, True), fill=color)
    label(draw, (40, 106), "tracé authored (cyan)", size=21, color=(205, 240, 255))
    label(draw, (40, 440), "le dirigeable coupe à l'intérieur, le chasseur suit", size=18, color=(255, 226, 180))
    return base


def main():
    replacements = {
        "path.png": path_image(),
        "target.png": target_image(),
        "profile.png": profile_image(),
    }
    for name, image in replacements.items():
        if image is not None:
            save(image, name)
        else:
            print("skip", name, "(captures missing)")


if __name__ == "__main__":
    main()
