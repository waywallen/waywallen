#!/usr/bin/env python3
"""WebSocket + protobuf smoke client for the waywallen control plane.

Prereqs:
  - `protoc` on $PATH
  - `pip install protobuf websockets`

Usage:
  python3 scripts/ws_smoke.py [ws://host:port]
"""
import asyncio
import importlib.util
import os
import subprocess
import sys
import tempfile

try:
    import websockets
except ImportError:
    sys.exit("missing dep: pip install websockets protobuf")

HERE = os.path.dirname(os.path.abspath(__file__))
PROTO = os.path.abspath(os.path.join(HERE, "..", "proto", "control.proto"))


def compile_proto():
    out = tempfile.mkdtemp(prefix="waywallen_smoke_")
    subprocess.run(
        ["protoc", f"--python_out={out}", f"-I{os.path.dirname(PROTO)}", PROTO],
        check=True,
    )
    mod_path = os.path.join(out, "control_pb2.py")
    spec = importlib.util.spec_from_file_location("control_pb2", mod_path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


async def rpc(ws, pb, request_id, build):
    req = pb.Request()
    req.request_id = request_id
    build(req)
    await ws.send(req.SerializeToString())
    raw = await ws.recv()
    resp = pb.Response()
    resp.ParseFromString(raw)
    return resp


async def main():
    url = sys.argv[1] if len(sys.argv) > 1 else "ws://127.0.0.1:8080"
    pb = compile_proto()
    async with websockets.connect(url) as ws:
        r = await rpc(ws, pb, 1, lambda r: r.health.SetInParent())
        print("health →", pb.Status.Name(r.status), r.payload.WhichOneof("payload"), r.health)

        r = await rpc(ws, pb, 2, lambda r: r.renderer_plugin_list.SetInParent())
        print("plugin_list →", pb.Status.Name(r.status),
              "renderers=", len(r.renderer_plugin_list.renderers),
              "types=", list(r.renderer_plugin_list.supported_types))

        r = await rpc(ws, pb, 3, lambda r: r.wallpaper_scan.SetInParent())
        print("wallpaper_scan →", pb.Status.Name(r.status),
              "count=", r.wallpaper_scan.count if r.status == pb.OK else r.message)

        r = await rpc(ws, pb, 4, lambda r: r.wallpaper_list.SetInParent())
        print("wallpaper_list →", pb.Status.Name(r.status),
              "count=", r.wallpaper_list.count)

        r = await rpc(ws, pb, 5, lambda r: r.renderer_list.SetInParent())
        print("renderer_list →", pb.Status.Name(r.status),
              list(r.renderer_list.renderers))


if __name__ == "__main__":
    asyncio.run(main())
