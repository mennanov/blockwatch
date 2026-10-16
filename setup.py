# The hooks in .pre-commit-hooks.yaml use `language: python`, so pre-commit installs this package. It holds no code.
# Its dependency pulls a prebuilt blockwatch from PyPI, so a project that uses the hooks doesn't need Rust.
from setuptools import setup

setup(
    name='blockwatch-pre-commit-placeholder',
    version='0.0.0',
    # <block name="blockwatch-cli-version" same-as-pattern="blockwatch-cli==(?P<value>\d+\.\d+\.\d+)">
    install_requires=['blockwatch-cli==0.14.0'],
    # </block>
    # Without it, setuptools takes the directories under src/ for Python packages.
    packages=[],
)
