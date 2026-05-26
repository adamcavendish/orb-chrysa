allow_k8s_contexts("kind-orb-chrysa-tilt")

config.define_bool("skip_image_build", False, "Use a preloaded orb-chrysa-server:tilt image")
config.define_bool(
    "host_docker_trust",
    False,
    "Install host Docker trust, restart/reload Docker if needed, and require host docker push in smoke",
)
cfg = config.parse()
host_docker_trust = cfg.get("host_docker_trust", False)

if not cfg.get("skip_image_build", False):
    docker_build(
        "orb-chrysa-server:tilt",
        ".",
        dockerfile="Dockerfile",
        ignore=[
            "target",
            ".git",
            "tests/conformance/results",
            "crates/orb-chrysa-server/dashboard/node_modules",
            "crates/orb-chrysa-server/dashboard/dist",
        ],
    )
else:
    print("Skipping orb-chrysa-server image build; expecting orb-chrysa-server:tilt to be preloaded")

local_resource(
    "cert-manager",
    "tests/k8s/tilt/install-cert-manager.sh",
    deps=["tests/k8s/tilt/install-cert-manager.sh"],
)

local_resource(
    "local-ca",
    "tests/k8s/tilt/render-local-ca.sh | kubectl apply -f - && kubectl -n cert-manager wait --for=condition=Ready certificate/orb-chrysa-ca --timeout=180s",
    deps=["tests/k8s/tilt/render-local-ca.sh"],
    resource_deps=["cert-manager"],
)

k8s_yaml(local("tests/k8s/tilt/render-rustfs.sh"))
k8s_resource("rustfs", resource_deps=["local-ca"])
k8s_resource("rustfs-init", resource_deps=["rustfs"])

k8s_yaml(local("tests/k8s/tilt/render-kanidm.sh"))
k8s_resource("kanidm", resource_deps=["local-ca"])

local_resource(
    "kanidm-bootstrap",
    "tests/k8s/tilt/bootstrap-kanidm.sh",
    deps=["tests/k8s/tilt/bootstrap-kanidm.sh"],
    resource_deps=["kanidm"],
)

k8s_yaml(local("tests/k8s/tilt/render-helm.sh"))
k8s_resource(
    "orb-chrysa",
    resource_deps=["kanidm-bootstrap", "rustfs-init", "local-ca"],
)

local_resource(
    "node-trust",
    "tests/k8s/tilt/kind-node-trust.sh",
    deps=["tests/k8s/tilt/kind-node-trust.sh"],
    resource_deps=["orb-chrysa"],
)

smoke_deps = ["node-trust"]
smoke_cmd = "tests/k8s/tilt-full-smoke.sh"

if host_docker_trust:
    local_resource(
        "host-docker-trust",
        "tests/k8s/tilt/host-docker-trust.sh --wait-kubernetes",
        deps=["tests/k8s/tilt/host-docker-trust.sh"],
        resource_deps=["node-trust"],
    )
    smoke_deps = ["host-docker-trust"]
    smoke_cmd = "REQUIRE_HOST_DOCKER_PUSH=1 tests/k8s/tilt-full-smoke.sh"

local_resource(
    "full-smoke",
    smoke_cmd,
    deps=["tests/k8s/tilt-full-smoke.sh"],
    resource_deps=smoke_deps,
)
