#!/usr/bin/env python3
"""Generate the annotated help illustrations embedded in the UI.

The screenshots in the animation section are produced separately from the
running application; this script composes the scene illustrations from the
existing demo renders and draws the annotations. It is deterministic: running
it again reproduces the same files.

Usage: python3 scripts/make_help_images.py
"""

from __future__ import annotations

import math
import shutil
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "help"
W, H = 960, 540
HEADER = 88

FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
FONT_B = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
FONT_M = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"

INK = (238, 246, 255)
MUTED = (158, 178, 200)
ACCENT = (255, 209, 102)
CYAN = (92, 202, 255)
ORANGE = (255, 176, 92)
RED = (236, 92, 92)
GREEN = (96, 208, 128)
BLUE = (92, 148, 255)


def font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(FONT_B if bold else FONT, size)


def mono(size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(FONT_M, size)


def canvas(title: str, subtitle: str, background: Image.Image | None = None) -> Image.Image:
    base = Image.new("RGB", (W, H), (10, 15, 22))
    if background is not None:
        bg = cover(background, W, H).filter(ImageFilter.GaussianBlur(1.2))
        base.paste(bg, (0, 0))
        overlay = Image.new("RGBA", (W, H), (8, 12, 18, 150))
        base = Image.alpha_composite(base.convert("RGBA"), overlay).convert("RGB")
    else:
        draw = ImageDraw.Draw(base)
        for y in range(H):
            t = y / H
            color = (
                int(8 + 16 * t),
                int(12 + 22 * t),
                int(18 + 30 * t),
            )
            draw.line([(0, y), (W, y)], fill=color)
    draw = ImageDraw.Draw(base)
    draw.rectangle([0, 0, W, HEADER], fill=(9, 13, 19))
    draw.line([(0, HEADER), (W, HEADER)], fill=(46, 64, 84), width=1)
    draw.text((26, 8), "AIDE VISTAFORGE", font=font(13, True), fill=(116, 156, 196))
    draw.text((26, 22), title, font=font(30, True), fill=INK)
    if subtitle:
        draw.text((26, 56), subtitle, font=font(15), fill=MUTED)
    return base


def cover(image: Image.Image, width: int, height: int, zoom: float = 1.0, anchor=(0.5, 0.5)) -> Image.Image:
    image = image.convert("RGB")
    scale = max(width / image.width, height / image.height) * zoom
    resized = image.resize((max(1, int(image.width * scale)), max(1, int(image.height * scale))), Image.LANCZOS)
    left = int((resized.width - width) * anchor[0])
    top = int((resized.height - height) * anchor[1])
    left = max(0, min(left, resized.width - width))
    top = max(0, min(top, resized.height - height))
    return resized.crop((left, top, left + width, top + height))


def paste_cover(base: Image.Image, image: Image.Image, box: tuple[int, int, int, int], zoom=1.0, anchor=(0.5, 0.5)) -> tuple[int, int, int, int]:
    x0, y0, x1, y1 = box
    fitted = cover(image, x1 - x0, y1 - y0, zoom, anchor)
    base.paste(fitted, (x0, y0))
    return box


def rounded_panel(base: Image.Image, box, radius=10, fill=(8, 12, 17, 190), outline=(58, 78, 100, 230)):
    overlay = Image.new("RGBA", base.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(overlay)
    d.rounded_rectangle(box, radius=radius, fill=fill, outline=outline, width=1)
    base.alpha_composite(overlay) if base.mode == "RGBA" else base.paste(
        Image.alpha_composite(base.convert("RGBA"), overlay).convert("RGB"), (0, 0)
    )


def label(draw: ImageDraw.ImageDraw, xy, text, *, size=22, color=INK, bg=(8, 12, 17, 220), border=(70, 94, 120), pad=7, anchor="lt"):
    f = font(size, True)
    bbox = draw.textbbox((0, 0), text, font=f)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    x, y = xy
    if "r" in anchor:
        x -= tw + pad * 2
    if "c" in anchor:
        x -= (tw + pad * 2) / 2
    if "b" in anchor:
        y -= th + pad * 2
    rect = (x, y, x + tw + pad * 2, y + th + pad * 2 + 2)
    draw.rounded_rectangle(rect, radius=6, fill=bg, outline=border, width=1)
    draw.text((x + pad, y + pad), text, font=f, fill=color)
    return (x, y, x + tw + pad * 2, y + th + pad * 2 + 2)


def arrow(draw: ImageDraw.ImageDraw, start, end, color=ACCENT, width=4):
    draw.line([start, end], fill=color, width=width)
    angle = math.atan2(end[1] - start[1], end[0] - start[0])
    length = 14
    left = (end[0] - length * math.cos(angle - 0.42), end[1] - length * math.sin(angle - 0.42))
    right = (end[0] - length * math.cos(angle + 0.42), end[1] - length * math.sin(angle + 0.42))
    draw.polygon([end, left, right], fill=color)


def badge(draw: ImageDraw.ImageDraw, xy, text, color=(28, 82, 62), text_color=(210, 255, 228)):
    f = font(14, True)
    bbox = draw.textbbox((0, 0), text, font=f)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    x, y = xy
    draw.rounded_rectangle((x, y, x + tw + 18, y + th + 12), radius=12, fill=color)
    draw.text((x + 9, y + 5), text, font=f, fill=text_color)


def pair(base, left, right, left_label, right_label, box=(24, 100, 936, 500)):
    x0, y0, x1, y1 = box
    gap = 8
    w = (x1 - x0 - gap) // 2
    paste_cover(base, left, (x0, y0, x0 + w, y1))
    paste_cover(base, right, (x0 + w + gap, y0, x1, y1))
    draw = ImageDraw.Draw(base, "RGBA")
    badge(draw, (x0 + 12, y0 + 12), left_label, (58, 34, 34), (255, 210, 210))
    badge(draw, (x0 + w + gap + 12, y0 + 12), right_label, (28, 58, 80), (210, 235, 255))
    draw.line([(x0 + w + gap // 2, y0), (x0 + w + gap // 2, y1)], fill=(255, 255, 255, 70), width=1)


def load(rel: str) -> Image.Image:
    return Image.open(ROOT / rel).convert("RGB")


def save(image: Image.Image, name: str):
    OUT.mkdir(parents=True, exist_ok=True)
    image.save(OUT / name, optimize=True)
    print("wrote", OUT / name)


def scene(title: str, subtitle: str, rel: str, zoom=1.0, anchor=(0.5, 0.5)) -> Image.Image:
    base = canvas(title, subtitle)
    paste_cover(base, load(rel), (24, 92, 936, 516), zoom, anchor)
    return base


def workspace():
    base = canvas("Un espace = un métier", "le viewport reste, le panneau change")
    paste_cover(base, load("docs/screenshots/alpine.png"), (24, 94, 936, 516))
    draw = ImageDraw.Draw(base, "RGBA")
    tabs = ["Terrain", "Modificateurs", "Roche", "Détail", "Sculpter", "Animation", "Érosion", "Eau"]
    x = 30
    for tab in tabs:
        f = font(14, True)
        tw = draw.textlength(tab, font=f)
        active = tab == "Animation"
        fill = (46, 108, 148, 235) if active else (20, 26, 36, 225)
        draw.rounded_rectangle((x, 96, x + tw + 20, 126), radius=8, fill=fill, outline=(80, 110, 140, 220))
        draw.text((x + 10, 102), tab, font=f, fill=(240, 248, 255) if active else (190, 204, 220))
        x += tw + 28
    label(draw, (250, 168), "Animation réunit tracé, vol et lecture", size=19, color=(255, 235, 185))
    arrow(draw, (360, 220), (430, 170), ACCENT, 4)
    label(draw, (610, 452), "Le panneau change, la scène reste", size=15, color=MUTED, bg=(8, 12, 17, 200))
    return base


def gizmo():
    base = canvas("Cadrer sans perdre le tracé", "le cube est solidaire de la caméra")
    paste_cover(base, load("demo-out/qual/patagonia_hero3.png"), (24, 94, 936, 516))
    src = load("docs/help/gizmo_source.png").resize((300, 365), Image.LANCZOS)
    base.paste(src, (618, 122))
    draw = ImageDraw.Draw(base, "RGBA")
    arrow(draw, (560, 230), (660, 220), ACCENT, 4)
    label(draw, (330, 170), "face cliquable", size=16)
    arrow(draw, (520, 300), (700, 360), CYAN, 4)
    label(draw, (330, 320), "maison = cadrage par défaut", size=16, color=(210, 240, 255))
    label(draw, (330, 440), "1 face · 2 dessus · 3 droite · 0 défaut", size=15, color=MUTED)
    return base


def path():
    base = canvas("Tracé de l'œil et point de vue", "gizmo de translation sur la clé sélectionnée")
    src = load("docs/help/path_source.png")
    paste_cover(base, src, (24, 92, 936, 516), zoom=1.0, anchor=(0.5, 0.56))
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (36, 100), "E2 · 4.0 s", size=15, color=(200, 240, 255))
    label(draw, (700, 100), "viseur du point de vue", size=15, color=(255, 220, 170))
    return base


def target():
    base = canvas("Ce que la caméra regarde", "la cible n'apparaît qu'en vue design")
    src = load("docs/help/path_source.png")
    paste_cover(base, src, (24, 92, 936, 516), zoom=1.5, anchor=(0.33, 0.52))
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (44, 110), "œil (tracé cyan)", size=16, color=(200, 240, 255))
    label(draw, (640, 140), "cible orange", size=16, color=(255, 220, 170))
    arrow(draw, (610, 200), (520, 280), ORANGE, 4)
    label(draw, (550, 420), "ligne œil → cible", size=15, color=(255, 220, 170))
    return base


def timing():
    base = canvas("Durée courte = corps saturé", "la durée change la vitesse, pas le tracé")
    draw = ImageDraw.Draw(base, "RGBA")
    area = (70, 120, 890, 470)
    draw.rounded_rectangle(area, radius=12, fill=(11, 16, 24, 235), outline=(56, 76, 98, 230))
    ox, oy = area[0] + 40, area[3] - 36
    ex, ey = area[2] - 30, area[1] + 30
    draw.line([(ox, ey), (ox, oy), (ex, oy)], fill=(120, 144, 168), width=2)
    # Speed demanded vs body limit.
    f = font(13)
    draw.text((ox + 8, area[1] + 8), "vitesse", font=f, fill=MUTED)
    draw.text((ex - 76, oy + 8), "temps", font=f, fill=MUTED)
    limit_y = area[1] + 120
    draw.line([(ox, limit_y), (ex, limit_y)], fill=(255, 176, 100), width=2)
    draw.text((ex - 150, limit_y - 22), "max du profil", font=font(13, True), fill=(255, 176, 100))
    # Demanded curve, saturating.
    pts = []
    for i in range(0, 101):
        t = i / 100
        x = ox + (ex - ox) * t
        v = 1.0 - math.exp(-3.2 * t) * 0.8
        y = oy - (oy - ey) * v
        pts.append((x, y))
    draw.line(pts, fill=(92, 202, 255), width=4, joint="curve")
    draw.text((ox + 18, oy - 26), "court", font=font(16, True), fill=INK)
    # Long duration curve.
    pts = []
    for i in range(0, 101):
        t = i / 100
        x = ox + (ex - ox) * t
        v = 0.52 * (1 - math.exp(-2.0 * t))
        y = oy - (oy - ey) * v
        pts.append((x, y))
    draw.line(pts, fill=(96, 208, 128), width=3, joint="curve")
    label(draw, (560, 330), "durée conseillée : fluide", size=15, color=(180, 255, 210))
    label(draw, (110, 155), "durée trop courte : saturation", size=15, color=(255, 210, 160))
    return base


def optics():
    base = canvas("FOV large ou téléobjectif", "même scène, sensation différente")
    paste_cover(base, load("demo-out/qual/patagonia_hero3.png"), (24, 92, 936, 516), 1.05)
    overlay = Image.new("RGBA", base.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay)
    eye = (150, 470)
    draw.ellipse([eye[0] - 8, eye[1] - 8, eye[0] + 8, eye[1] + 8], fill=(92, 202, 255, 255))
    draw.polygon([eye, (700, 160), (820, 360)], fill=(92, 202, 255, 42), outline=(92, 202, 255, 200))
    draw.polygon([eye, (650, 240), (700, 320)], fill=(255, 176, 92, 52), outline=(255, 176, 92, 220))
    draw.text((430, 130), "large 78° : vitesse", font=font(16, True), fill=(190, 232, 255))
    draw.text((700, 380), "serré 35° : compression", font=font(16, True), fill=(255, 220, 170))
    base = Image.alpha_composite(base.convert("RGBA"), overlay).convert("RGB")
    return base


def constraints():
    base = canvas("La trajectoire évite le sol et les parois", "correction douce, clés inchangées")
    draw = ImageDraw.Draw(base, "RGBA")
    # Ground profile.
    ground = [(40, 450), (180, 380), (320, 400), (470, 300), (620, 340), (780, 250), (930, 300), (930, 516), (40, 516)]
    draw.polygon(ground, fill=(42, 54, 48, 255), outline=(90, 110, 96, 255))
    # Authored path (straight, through the hill).
    draw.line([(80, 250), (900, 220)], fill=(92, 202, 255, 220), width=4)
    draw.text((90, 210), "tracé authored", font=font(15, True), fill=(190, 232, 255))
    # Simulated path (offset above ground).
    sim = [(90, 250), (200, 290), (330, 300), (470, 235), (620, 255), (770, 190), (890, 200)]
    draw.line(sim, fill=(255, 226, 120, 240), width=4, joint="curve")
    draw.text((600, 300), "trajectoire simulée", font=font(15, True), fill=(255, 235, 185))
    # Clearance.
    draw.line([(470, 235), (470, 300)], fill=(255, 176, 92, 230), width=2)
    draw.text((480, 252), "hauteur sol", font=font(14, True), fill=(255, 210, 160))
    # Wall push.
    arrow(draw, (780, 420), (700, 360), (236, 92, 92, 255), 5)
    draw.text((700, 440), "répulsion de paroi", font=font(15, True), fill=(255, 180, 180))
    label(draw, (40, 100), "les clés bleues ne bougent pas", size=16, color=(190, 232, 255))
    return base


def profile():
    base = canvas("Même tracé, cinq corps", "les trajectoires divergent selon le profil")
    paste_cover(base, load("demo-out/qual/patagonia_hero3.png"), (24, 92, 936, 516), 1.02)
    overlay = Image.new("RGBA", base.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay)
    colors = [(255, 138, 76), (120, 205, 255), (134, 224, 154), (210, 165, 255), (255, 92, 120)]
    names = ["Drone", "Oiseau", "Hélico", "Dirigeable", "Chasse"]
    for i, (color, name) in enumerate(zip(colors, names)):
        offset = (i - 2) * 16
        pts = []
        for j in range(0, 81):
            t = j / 80
            x = 80 + 820 * t
            y = 390 - 170 * math.sin(math.pi * t) + offset - 34 * math.sin(2.3 * math.pi * t)
            pts.append((x, y))
        draw.line(pts, fill=color + (220,), width=4)
        draw.text((780, 120 + i * 28), name, font=font(15, True), fill=color + (255,))
    base = Image.alpha_composite(base.convert("RGBA"), overlay).convert("RGB")
    label(ImageDraw.Draw(base, "RGBA"), (40, 100), "à tracé égal, le corps ne suit pas la même courbe", size=16)
    return base


def transport():
    base = canvas("Toujours là pour lancer la lecture", "la barre reste ancrée en bas du viewport")
    paste_cover(base, load("demo-out/qual/patagonia_hero3.png"), (24, 92, 936, 516), 1.05)
    overlay = Image.new("RGBA", base.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay)
    bar = (54, 406, 906, 492)
    draw.rounded_rectangle(bar, radius=12, fill=(12, 16, 23, 245), outline=(64, 88, 112, 255))
    draw.rounded_rectangle((70, 418, 148, 458), radius=6, fill=(38, 82, 62, 255))
    draw.text((86, 426), "▶ Lire", font=font(18, True), fill=(230, 255, 240))
    draw.rounded_rectangle((162, 418, 196, 458), radius=6, fill=(28, 34, 46, 255))
    draw.text((169, 424), "⏮", font=font(18, True), fill=INK)
    draw.line([(215, 438), (720, 438)], fill=(80, 100, 122), width=4)
    draw.ellipse([(370, 428), (390, 448)], fill=(255, 176, 92))
    draw.text((404, 452), "t = 17.58 s", font=mono(14), fill=(230, 240, 250))
    draw.rounded_rectangle((740, 418, 812, 458), radius=6, fill=(34, 64, 88, 255))
    draw.text((752, 426), "Design", font=font(14, True), fill=(210, 235, 255))
    draw.rounded_rectangle((818, 418, 890, 458), radius=6, fill=(46, 108, 148, 255))
    draw.text((836, 426), "Caméra", font=font(14, True), fill=(240, 250, 255))
    draw.text((72, 472), "1207 images · 50.29 s · 24 fps · profil Drone FPV", font=font(13), fill=(160, 178, 196))
    draw.text((620, 472), "Espace : lire/pause · F1 : aide", font=font(13), fill=(140, 158, 178))
    base = Image.alpha_composite(base.convert("RGBA"), overlay).convert("RGB")
    arrow(ImageDraw.Draw(base, "RGBA"), (500, 380), (410, 420), ACCENT, 4)
    return base


def terrain():
    base = scene("Générer un relief", "mode, taille, graine, preset", "docs/screenshots/alpine.png", 1.03)
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (36, 100), "514² pour travailler", size=16)
    label(draw, (660, 100), "1026² pour le rendu final", size=16, color=(255, 220, 170))
    return base


def world():
    base = scene("Tuiler sans couture", "la variation macro casse la répétition", "demo-out/world/seamless3x3.png", 1.02)
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (40, 104), "bords raccordés (C0)", size=16, color=(180, 255, 210))
    arrow(draw, (300, 160), (230, 250), ACCENT, 4)
    label(draw, (560, 430), "même tuile, orientations variées", size=15, color=(210, 230, 250))
    return base


def modifiers():
    base = canvas("Une pile, des signatures", "l'ordre des opérateurs compte")
    src = load("demo-out/modifiers/gallery.png")
    # Keep the top two rows of the gallery.
    cropped = src.crop((0, 0, src.width, int(src.height * 0.62)))
    paste_cover(base, cropped, (24, 100, 936, 516), 1.0, (0.5, 0.1))
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (36, 108), "Terrasses", size=15)
    label(draw, (500, 108), "Érosion", size=15)
    label(draw, (36, 300), "Cratères", size=15)
    label(draw, (500, 300), "Congères", size=15)
    return base


def rock():
    base = canvas("Bancs, fractures, dissolution", "la géologie passe après l'érosion")
    src = load("demo-out/geology/galerie_roches.png")
    paste_cover(base, src, (24, 100, 936, 516), 1.05, (0.5, 0.42))
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (36, 108), "granite", size=16)
    label(draw, (500, 108), "calcaire", size=16)
    label(draw, (36, 300), "grès", size=16)
    label(draw, (500, 300), "craie", size=16)
    return base


def detail():
    base = canvas("Le grain de la roche", "détail fractal + maillage subdivisé")
    pair(base, load("demo-out/detail/rock_comparaison.png").crop((0, 26, 1000, 744)), load("demo-out/detail/rock_comparaison.png").crop((1000, 26, 2000, 744)), "SANS DÉTAIL", "AVEC DÉTAIL")
    return base


def sculpt():
    base = canvas("Sculpter localement", "chaque trait est annulable par région")
    src = load("demo-out/sculpt/comparaison.png")
    half = src.width // 2
    pair(base, src.crop((0, 0, half, src.height)), src.crop((half, 0, src.width, src.height)), "AVANT", "APRÈS")
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (530, 470), "rayon, forme, symétrie", size=15, color=(210, 235, 255))
    return base


def erosion():
    base = canvas("Ravines et dépôts", "l'érosion précède la géologie")
    src = load("demo-out/geology/avant_apres.png")
    half = src.width // 2
    pair(base, src.crop((0, 30, half, src.height)), src.crop((half, 30, src.width, src.height)), "AVANT", "APRÈS")
    return base


def water():
    base = scene("Le niveau de mer noie les points bas", "vagues, opacité, écume", "docs/screenshots/island.png", 1.0)
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (36, 104), "niveau de mer", size=16, color=(190, 232, 255))
    arrow(draw, (180, 150), (360, 350), ACCENT, 4)
    label(draw, (600, 460), "hauts-fonds visibles", size=15, color=(180, 255, 210))
    return base


def materials():
    base = canvas("Neige, éboulis, végétation", "des masques guidés par l'altitude et la pente")
    paste_cover(base, load("docs/screenshots/detail_rock.png"), (24, 100, 936, 516), 1.15, (0.35, 0.35))
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (36, 108), "neige en plaques", size=16, color=(220, 242, 255))
    label(draw, (520, 108), "roche nue", size=16, color=(240, 220, 190))
    label(draw, (36, 420), "éboulis en pied de pente", size=15, color=(230, 210, 180))
    return base


def light():
    base = scene("Heure et ambiance", "soleil, palette, atmosphère, exposition", "docs/screenshots/dusk.png", 1.04)
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (36, 104), "soleil rasant + étoiles", size=16, color=(255, 220, 170))
    label(draw, (600, 460), "palette dusk", size=15, color=(255, 200, 150))
    return base


def camera():
    base = canvas("Vues CAO", "indépendantes de la caméra animée")
    paste_cover(base, load("docs/screenshots/alpine.png"), (24, 94, 936, 516))
    src = load("docs/help/gizmo_source.png").resize((260, 316), Image.LANCZOS)
    base.paste(src, (650, 140))
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (330, 140), "1 face · 2 dessus · 3 droite", size=16)
    arrow(draw, (560, 200), (700, 265), ACCENT, 4)
    label(draw, (330, 430), "la vue design mémorise votre cadrage", size=15, color=(210, 235, 255))
    return base


def render():
    base = scene("Le rendu de référence", "CPU déterministe, indépendant du GPU", "demo-out/qual/patagonia_hero3.png", 1.02)
    draw = ImageDraw.Draw(base, "RGBA")
    label(draw, (36, 104), "Render", size=16, color=(255, 220, 170))
    label(draw, (36, 440), "Fast · Refined · Photoreal", size=15)
    label(draw, (610, 440), "SSAA, ombres, AO", size=15, color=(190, 232, 255))
    return base


def main():
    # Preserve the raw captures used to compose several illustrations.
    for name in ("gizmo", "path"):
        source = OUT / f"{name}.png"
        backup = OUT / f"{name}_source.png"
        if source.exists() and not backup.exists():
            shutil.copyfile(source, backup)

    save(workspace(), "workspace.png")
    save(gizmo(), "gizmo.png")
    save(path(), "path.png")
    save(target(), "target.png")
    save(timing(), "timing.png")
    save(optics(), "optics.png")
    save(constraints(), "constraints.png")
    save(profile(), "profile.png")
    save(transport(), "transport.png")
    save(terrain(), "terrain.png")
    save(world(), "world.png")
    save(modifiers(), "modifiers.png")
    save(rock(), "rock.png")
    save(detail(), "detail.png")
    save(sculpt(), "sculpt.png")
    save(erosion(), "erosion.png")
    save(water(), "water.png")
    save(materials(), "materials.png")
    save(light(), "light.png")
    save(camera(), "camera.png")
    save(render(), "render.png")


if __name__ == "__main__":
    main()
