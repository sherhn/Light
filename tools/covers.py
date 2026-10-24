"""Export quiet module covers shared by the IDE and the standalone Light website.
Uses local SVG marks and sharp; set NODE_PATH to an existing sharp installation.
"""
import pathlib
import subprocess


def draw(module, path):
    script = pathlib.Path(__file__).with_name("module-covers.mjs")
    subprocess.run(["node", str(script), module["id"], str(path)], check=True)
