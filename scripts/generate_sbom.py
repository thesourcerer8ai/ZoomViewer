#!/usr/bin/env python3
"""Generate a CycloneDX 1.5 JSON SBOM from `cargo metadata` and `Cargo.lock`."""

import argparse
import datetime
import json
import pathlib
import re
import subprocess
import sys
import uuid


def parse_lockfile_checksums(lockfile_path: pathlib.Path) -> dict[tuple[str, str], str]:
    """Return a mapping of (package_name, version) -> sha256 checksum from Cargo.lock."""
    checksums: dict[tuple[str, str], str] = {}
    if not lockfile_path.exists():
        return checksums

    content = lockfile_path.read_text(encoding="utf-8")
    for block in content.split("[[package]]"):
        m_name = re.search(r'^name\s*=\s*"([^"]+)"', block, re.M)
        m_ver = re.search(r'^version\s*=\s*"([^"]+)"', block, re.M)
        m_sum = re.search(r'^checksum\s*=\s*"([^"]+)"', block, re.M)
        if m_name and m_ver and m_sum:
            checksums[(m_name.group(1), m_ver.group(1))] = m_sum.group(1)
    return checksums


def make_component(
    pkg: dict,
    is_root: bool,
    checksums: dict[tuple[str, str], str],
) -> dict:
    """Convert a `cargo metadata` package entry into a CycloneDX component."""
    name = pkg["name"]
    version = pkg["version"]
    comp: dict = {
        "type": "application" if is_root else "library",
        "bom-ref": pkg["id"],
        "name": name,
        "version": version,
        "purl": f"pkg:cargo/{name}@{version}",
    }
    if pkg.get("description"):
        comp["description"] = pkg["description"].strip()
    if pkg.get("license"):
        comp["licenses"] = [{"expression": pkg["license"]}]
    if (name, version) in checksums:
        comp["hashes"] = [{"alg": "SHA-256", "content": checksums[(name, version)]}]

    ext_refs = []
    if pkg.get("repository"):
        ext_refs.append({"type": "vcs", "url": pkg["repository"]})
    if pkg.get("homepage"):
        ext_refs.append({"type": "website", "url": pkg["homepage"]})
    if ext_refs:
        comp["externalReferences"] = ext_refs

    return comp


def generate_sbom(manifest_dir: pathlib.Path, output_path: pathlib.Path) -> tuple[int, int]:
    """Run `cargo metadata` and write a CycloneDX 1.5 JSON SBOM to `output_path`."""
    meta_raw = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=manifest_dir,
        text=True,
    )
    meta = json.loads(meta_raw)

    checksums = parse_lockfile_checksums(manifest_dir / "Cargo.lock")
    root_id = meta["resolve"]["root"]
    pkg_by_id = {p["id"]: p for p in meta["packages"]}
    root_pkg = pkg_by_id[root_id]

    components = [
        make_component(p, is_root=False, checksums=checksums)
        for p in sorted(meta["packages"], key=lambda p: (p["name"], p["version"]))
        if p["id"] != root_id
    ]
    dependencies = [
        {"ref": node["id"], "dependsOn": sorted(node.get("dependencies", []))}
        for node in sorted(meta["resolve"]["nodes"], key=lambda n: n["id"])
    ]

    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{uuid.uuid4()}",
        "version": 1,
        "metadata": {
            "timestamp": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "tools": {
                "components": [
                    {"type": "application", "name": "cargo-metadata", "version": "1"}
                ]
            },
            "component": make_component(root_pkg, is_root=True, checksums=checksums),
        },
        "components": components,
        "dependencies": dependencies,
    }

    output_path.write_text(json.dumps(sbom, indent=2) + "\n", encoding="utf-8")
    return len(components), len(dependencies)


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate CycloneDX 1.5 JSON SBOM for Cargo project.")
    parser.add_argument(
        "-o",
        "--output",
        default="sbom.cdx.json",
        help="Output SBOM path (default: sbom.cdx.json)",
    )
    parser.add_argument(
        "--manifest-dir",
        default=".",
        help="Directory containing Cargo.toml and Cargo.lock (default: .)",
    )
    args = parser.parse_args()

    manifest_dir = pathlib.Path(args.manifest_dir).resolve()
    output_path = pathlib.Path(args.output)
    if not output_path.is_absolute():
        output_path = manifest_dir / output_path

    try:
        comp_count, dep_count = generate_sbom(manifest_dir, output_path)
    except subprocess.CalledProcessError as exc:
        print(f"Error running cargo metadata: {exc}", file=sys.stderr)
        return exc.returncode or 1

    print(f"Updated {output_path.name}: {comp_count} components, {dep_count} dependency nodes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

