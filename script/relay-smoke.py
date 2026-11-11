#!/usr/bin/env python3
"""Exercise a release Compose in disposable projects; never use a user's volume."""
import argparse
import base64
import hashlib
import http.client
import json
import pathlib
import socket
import ssl
import subprocess
import tempfile
import time
import uuid


def run(*args):
    return subprocess.check_output(args, text=True).strip()


class TrustedConnection(http.client.HTTPSConnection):
    def connect(self):
        raw = socket.create_connection(("127.0.0.1", self.port), timeout=10)
        self.sock = self._context.wrap_socket(raw, server_hostname=self.host)


def request(pairing, port, method, path, body=None, headers=None, auth=True):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.load_verify_locations(cadata=pairing["ca_pem"])
    connection = TrustedConnection(pairing["logical_name"], port, context=context, timeout=10)
    fields = dict(headers or {})
    if auth:
        fields["Authorization"] = f'Bearer {pairing["secret"]}'
    try:
        connection.request(method, path, body=body, headers=fields)
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()


def check(condition, message):
    if not condition:
        raise RuntimeError(message)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("compose", type=pathlib.Path)
    args = parser.parse_args()
    source = args.compose.resolve()
    suffix = uuid.uuid4().hex[:12]
    projects = [f"easycopy-smoke-{suffix}-a", f"easycopy-smoke-{suffix}-b"]
    with tempfile.TemporaryDirectory(prefix="easycopy-relay-smoke-") as directory:
        # Copy the actual distribution Compose, changing only its host binding.
        # The named volume remains controlled by our unique project name.
        compose = pathlib.Path(directory) / "compose.yaml"
        content = source.read_text()
        check(content.count('${EASYCOPY_PORT:-8443}:8443') == 1, "unexpected Compose port mapping")
        compose.write_text(content.replace('${EASYCOPY_PORT:-8443}:8443', '127.0.0.1::8443'))

        def dc(project, *arguments):
            return run("docker", "compose", "-p", project, "-f", str(compose), *arguments)

        def start(project, recreate=False):
            extra = ["--force-recreate"] if recreate else []
            dc(project, "up", "-d", "--no-build", "--pull", "never", *extra)
            container = dc(project, "ps", "-q", "relay")
            for _ in range(90):
                state = json.loads(run("docker", "inspect", container))[0]
                if state["State"].get("Health", {}).get("Status") == "healthy":
                    break
                check(state["State"]["Running"], "relay stopped before becoming healthy")
                time.sleep(1)
            else:
                raise RuntimeError("relay did not become healthy within 90 seconds")
            check(state["Config"]["User"] == "10001:10001", "relay must run as non-root")
            check(state["HostConfig"]["ReadonlyRootfs"], "root filesystem must be read-only")
            check("ALL" in state["HostConfig"]["CapDrop"], "capabilities must be dropped")
            check("no-new-privileges:true" in state["HostConfig"]["SecurityOpt"], "security option missing")
            check(state["HostConfig"]["RestartPolicy"]["Name"] == "unless-stopped", "restart policy missing")
            check(state["HostConfig"]["LogConfig"]["Config"].get("max-file") == "3", "log rotation missing")
            dc(project, "exec", "-T", "relay", "easycopy-relay", "healthcheck")
            encoded = dc(project, "exec", "-T", "relay", "easycopy-relay", "pairing")
            check(encoded.startswith("easycopy1:"), "pairing output invalid")
            raw = encoded.removeprefix("easycopy1:")
            pairing = json.loads(base64.urlsafe_b64decode(raw + "=" * (-len(raw) % 4)))
            port = int(dc(project, "port", "relay", "8443").rsplit(":", 1)[1])
            return pairing, port

        try:
            config = json.loads(dc(projects[0], "config", "--format", "json"))
            image = config["services"]["relay"]["image"]
            metadata = json.loads(run("docker", "image", "inspect", image))[0]
            check((metadata["Os"], metadata["Architecture"]) == ("linux", "amd64"), "incorrect image architecture")
            pairing, port = start(projects[0])
            status, body = request(pairing, port, "GET", "/v1/info")
            check(status == 200 and json.loads(body)["server_id"] == pairing["server_id"], "trusted TLS/info failed")
            check(request(pairing, port, "GET", "/v1/info", auth=False)[0] == 401, "missing authentication accepted")
            invalid = dict(pairing, secret="wrong-secret")
            check(request(invalid, port, "GET", "/v1/info")[0] == 401, "incorrect authentication accepted")
            payload = ("EasyCopy 容器发行验证\n" * 4096).encode()
            digest = hashlib.sha256(payload).hexdigest()
            item_id = str(uuid.uuid4())
            intent = dict(id=item_id, device_id=str(uuid.uuid4()), device_name="release smoke test", kind="text",
                          created_at=int(time.time() * 1000), preview="release validation")
            status, body = request(pairing, port, "POST", "/v1/items", json.dumps(intent).encode(), {"Content-Type": "application/json"})
            check(status == 200 and json.loads(body)["id"] == item_id, "registration failed")
            path = f"/v1/items/{item_id}/payload"
            status, body = request(pairing, port, "PUT", path, payload,
                                   {"x-easycopy-size": str(len(payload)), "x-easycopy-sha256": digest})
            check(status == 200 and json.loads(body)["status"] == "complete", "upload failed")
            status, downloaded = request(pairing, port, "GET", path)
            check(status == 200 and downloaded == payload, "download differs")
            print("PASS: linux/amd64, non-root first boot, health, trusted TLS, authentication and transfer")
            other, _ = start(projects[1])
            check(other["server_id"] != pairing["server_id"] and other["secret"] != pairing["secret"], "image contains a shared identity")
            print("PASS: independent empty volumes generate different identities")
            restored, port = start(projects[0], recreate=True)
            check(restored == pairing, "identity changed after container replacement")
            status, body = request(pairing, port, "GET", f"/v1/items/{item_id}")
            check(status == 200 and json.loads(body)["sha256"] == digest, "metadata lost after container replacement")
            status, downloaded = request(pairing, port, "GET", path)
            check(status == 200 and hashlib.sha256(downloaded).hexdigest() == digest, "payload lost after container replacement")
            print("PASS: original pairing, metadata and payload survive container replacement")
        finally:
            for project in projects:
                # Only these randomly named smoke projects are ever removed.
                dc(project, "down", "--volumes", "--remove-orphans")


if __name__ == "__main__":
    main()
