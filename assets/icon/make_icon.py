"""Export the approved Light SVG. Requires Node.js and sharp (also accepts NODE_PATH)."""
import pathlib
import subprocess

if __name__ == "__main__":
    subprocess.run(["node", str(pathlib.Path(__file__).with_suffix(".mjs"))], check=True)
