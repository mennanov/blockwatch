# The hooks in .pre-commit-hooks.yaml use `language: python`, so pre-commit installs this package. It holds no code.
# Its dependency pulls a prebuilt blockwatch from PyPI, so a project that uses the hooks doesn't need Rust.
from setuptools import setup

setup(
    name='blockwatch-pre-commit-placeholder',
    version='0.0.0',
    install_requires=['blockwatch-cli==0.11.0'],
    # Without it, setuptools takes the directories under src/ for Python packages.
    packages=[],
)
