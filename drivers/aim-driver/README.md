To use the AIM driver install the required pacakges by running `uv sync`.

Start a local AIM server with: `uv run aim up`.

Start the communication driver for aim with: `uv run uvicorn main:app --port 61000 --reload`.

Make sure that the schedulers' config contains an entry for AIM. `status_bridge = "0.0.0.0:61000"` it points 
to the IP and port for the communication driver. Also ensure to open the local AIM dashboard in your browser.
'ttp://127.0.0.1:43800'