from aim.sdk.run import Run
from fastapi import FastAPI, Request

app = FastAPI()
run = Run(capture_terminal_logs=False)


@app.post("/status")
async def update(request: Request) -> None:
    json = await request.json()
    run.track(
        name=f"{json.get('worker_id')}_{json.get('metric_name')}", epoch=int(json.get("round")), value=json.get("value")
    )
