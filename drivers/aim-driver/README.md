# Hypha AIM Driver and Communication Driver

This README provides instructions on how to use the Hypha AIM Driver and Communication Driver.

## Installation

- Install the required packages by running `uv sync`.
- Start a local AIM server with: `uv run aim up`. Keep the Terminal open.
- In a new Terminal window, start the communication driver for aim with: `uv run uvicorn main:app --port 61000 --reload`. Keep the Terminal open.

## Scheduler Configuration

You're all set up with the Hypha AIM Driver and Communication Driver now. Head over to your Hypha installation and configure the Scheduler to use the AIM Communication Driver.
Add `status_bridge = "127.0.0.1:61000"` to the schedulers config.

## View Metrics

After the Scheduler is configured, you can view the AIM dashboard in your browser by going to http://127.0.0.1:43800
