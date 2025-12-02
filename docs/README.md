# Hypha Documentation

This directory contains the source code for the Hypha documentation website, built using [Zola](https://www.getzola.org/).

## Prerequisites

You need to install **Zola** to build and serve the documentation locally.

Please refer to the official Zola documentation for installation instructions for your operating system:
[https://www.getzola.org/documentation/getting-started/installation/](https://www.getzola.org/documentation/getting-started/installation/)

## Running Locally

To serve the documentation locally with live reload, run the following command from the repository root:

```bash
zola --root docs serve
```

Alternatively, you can navigate into the `docs` directory and run:

```bash
cd docs
zola serve
```

The site will be available at `http://127.0.0.1:1111` by default.
