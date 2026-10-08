import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { getPublicWorkspacePackages } from "./release-packages.mjs";
import { execNpmSync, parseNpmPackResult } from "./npm-command.mjs";

const manifestSchemaVersion = 1;

function isInsidePath(child, parent) {
	const relativePath = relative(parent, child);
	return relativePath === "" || (!relativePath.startsWith("..") && !isAbsolute(relativePath));
}

function getGitSource(repoRoot) {
	const commit = execFileSync("git", ["rev-parse", "HEAD"], { cwd: repoRoot, encoding: "utf8" }).trim();
	const status = execFileSync("git", ["status", "--porcelain"], { cwd: repoRoot, encoding: "utf8" });
	return { commit, dirty: status.length > 0 };
}

function packPackages(packages, tarballDirectory) {
	mkdirSync(tarballDirectory, { recursive: true });
	const packedPackages = [];
	for (const pkg of packages) {
		const packageJson = JSON.parse(readFileSync(join(pkg.directory, "package.json"), "utf8"));
		if (packageJson.name !== pkg.name) throw new Error(`Unexpected package name in ${pkg.directory}`);
		const output = execNpmSync(["pack", "--ignore-scripts", "--json", "--pack-destination", tarballDirectory], {
			cwd: pkg.directory,
			encoding: "utf8",
			stdio: ["inherit", "pipe", "inherit"],
		});
		const packed = parseNpmPackResult(output, pkg.name);
		if (packed.version !== packageJson.version || (pkg.version !== undefined && pkg.version !== packed.version)) {
			throw new Error(`Unexpected package version for ${pkg.name}`);
		}
		const files = new Set(packed.files.map((file) => file.path));
		const required = [
			"package.json",
			packageJson.main,
			packageJson.types,
			...Object.values(typeof packageJson.bin === "string" ? { bin: packageJson.bin } : (packageJson.bin ?? {})),
			...(packageJson.bundleDependencies ?? []).map((name) => `node_modules/${name}/package.json`),
		];
		if (required.filter(Boolean).some((file) => !files.has(file.replace(/^\.\//, "")))) {
			throw new Error(`${pkg.name} package contents are incomplete or do not match the manifest`);
		}

		const originalPath = join(tarballDirectory, packed.filename);
		const contents = readFileSync(originalPath);
		const digest = createHash("sha512").update(contents).digest();
		const hash = digest.toString("hex").slice(0, 12);
		const tarballPath = originalPath.replace(/\.tgz$/, `-${hash}.tgz`);
		renameSync(originalPath, tarballPath);
		console.log(
			`  ${pkg.name}@${packed.version}: ${packed.files.length} files, ${packed.size} bytes packed, ${packed.unpackedSize} bytes unpacked`,
		);
		packedPackages.push({
			name: pkg.name,
			version: packed.version,
			tarballPath,
			integrity: `sha512-${digest.toString("base64")}`,
		});
	}
	return packedPackages;
}

function prepareBundledWorkspaces(repoRoot, packages) {
	const hosts = packages
		.map((pkg) => ({ ...pkg, manifest: JSON.parse(readFileSync(join(pkg.directory, "package.json"), "utf8")) }))
		.filter(
			(pkg) =>
				pkg.manifest.publishConfig?.registry === "https://npm.pkg.github.com" &&
				pkg.manifest.bundleDependencies?.length > 0,
		);
	if (hosts.length === 0) return;
	const workspaces = getPublicWorkspacePackages(join(repoRoot, "packages"));
	for (const host of hosts) {
		for (const name of host.manifest.bundleDependencies) {
			const dependency = workspaces.find((pkg) => pkg.name === name);
			if (!dependency) throw new Error(`Missing bundled workspace ${name}`);
			const manifest = JSON.parse(readFileSync(join(dependency.directory, "package.json"), "utf8"));
			for (const [external, version] of Object.entries(manifest.dependencies ?? {})) {
				if (host.manifest.dependencies?.[external] !== version) {
					throw new Error(`${host.name} must declare ${external}@${version} for bundled ${name}`);
				}
			}
			const output = execNpmSync(["pack", "--dry-run", "--ignore-scripts", "--json"], {
				cwd: dependency.directory,
				encoding: "utf8",
			});
			const packed = parseNpmPackResult(output, name);
			if (packed.version !== dependency.version) throw new Error(`Unexpected package version for ${name}`);
			const target = join(host.directory, "node_modules", name);
			rmSync(target, { recursive: true, force: true });
			for (const file of packed.files) {
				const destination = join(target, file.path);
				mkdirSync(dirname(destination), { recursive: true });
				cpSync(join(dependency.directory, file.path), destination);
			}
			console.log(`Bundled ${name}@${dependency.version} into ${host.name}`);
		}
	}
}

function prepareOutputDirectory(outDir, options) {
	const repoRoot = resolve(options.repoRoot);
	if (!outDir) return mkdtempSync(join(tmpdir(), "pi-package-artifacts-"));
	const outputDirectory = resolve(outDir);
	if (dirname(outputDirectory) === outputDirectory || isInsidePath(repoRoot, outputDirectory)) {
		throw new Error(
			`Output directory must not be the repository, its ancestor, or a filesystem root: ${outputDirectory}`,
		);
	}
	if (isInsidePath(outputDirectory, repoRoot) && !isInsidePath(outputDirectory, join(repoRoot, ".artifacts"))) {
		throw new Error(
			`Repository-local output directory must be inside ${join(repoRoot, ".artifacts")}: ${outputDirectory}`,
		);
	}
	if (existsSync(outputDirectory)) {
		if (!options.force)
			throw new Error(`Output directory already exists. Use --force to replace it: ${outputDirectory}`);
		rmSync(outputDirectory, { force: true, recursive: true });
	}
	mkdirSync(outputDirectory, { recursive: true });
	return outputDirectory;
}

export function produceArtifactSet({
	repoRoot,
	outDir,
	build = true,
	offlineModelData = false,
	force = false,
	source,
	packages,
}) {
	const root = resolve(repoRoot);
	const artifactDirectory = prepareOutputDirectory(outDir, { force, repoRoot: root });
	try {
		if (build) {
			execNpmSync(["run", "clean"], { cwd: root, stdio: "inherit" });
			execNpmSync(["run", offlineModelData ? "build:offline" : "build"], { cwd: root, stdio: "inherit" });
		}
		const selectedPackages = packages ?? getPublicWorkspacePackages(join(root, "packages"));
		if (selectedPackages.length === 0) throw new Error("At least one package is required for an artifact set");
		if (new Set(selectedPackages.map((pkg) => pkg.name)).size !== selectedPackages.length) {
			throw new Error("Duplicate package selection for an artifact set");
		}
		prepareBundledWorkspaces(root, selectedPackages);
		const artifactSource = source === undefined ? getGitSource(root) : source;
		const packedPackages = packPackages(selectedPackages, join(artifactDirectory, "tarballs"));
		const manifest = {
			schemaVersion: manifestSchemaVersion,
			source: artifactSource,
			packages: packedPackages
				.map(({ name, version, tarballPath, integrity }) => ({
					name,
					version,
					tarball: relative(artifactDirectory, tarballPath).replaceAll("\\", "/"),
					integrity,
				}))
				.sort((left, right) => left.name.localeCompare(right.name)),
		};
		const manifestPath = join(artifactDirectory, "manifest.json");
		writeFileSync(manifestPath, `${JSON.stringify(manifest, null, "\t")}\n`);
		return readArtifactSet(manifestPath);
	} catch (error) {
		rmSync(artifactDirectory, { force: true, recursive: true });
		throw error;
	}
}

export function readArtifactSet(manifestPath) {
	const absoluteManifestPath = resolve(manifestPath);
	const artifactDirectory = dirname(absoluteManifestPath);
	const manifestContents = readFileSync(absoluteManifestPath, "utf8");
	const manifest = JSON.parse(manifestContents);
	if (manifest.schemaVersion !== manifestSchemaVersion || !Array.isArray(manifest.packages)) {
		throw new Error(`Unsupported package artifact manifest: ${absoluteManifestPath}`);
	}
	if (
		manifest.source !== null &&
		(!manifest.source ||
			!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(manifest.source.commit) ||
			typeof manifest.source.dirty !== "boolean")
	) {
		throw new Error(`Invalid package artifact source: ${absoluteManifestPath}`);
	}
	const names = new Set();
	const packages = manifest.packages.map((pkg) => {
		if (
			typeof pkg?.name !== "string" ||
			typeof pkg.version !== "string" ||
			typeof pkg.tarball !== "string" ||
			typeof pkg.integrity !== "string"
		) {
			throw new Error(`Invalid package entry in artifact manifest: ${absoluteManifestPath}`);
		}
		if (names.has(pkg.name)) throw new Error(`Duplicate package in artifact manifest: ${pkg.name}`);
		names.add(pkg.name);
		const tarballPath = resolve(artifactDirectory, pkg.tarball);
		if (!isInsidePath(tarballPath, artifactDirectory) || !existsSync(tarballPath)) {
			throw new Error(`Missing package tarball for ${pkg.name}: ${tarballPath}`);
		}
		const integrity = `sha512-${createHash("sha512").update(readFileSync(tarballPath)).digest("base64")}`;
		if (integrity !== pkg.integrity)
			throw new Error(`Package tarball integrity mismatch for ${pkg.name}: ${tarballPath}`);
		return { ...pkg, tarballPath };
	});
	return {
		artifactDirectory,
		manifestPath: absoluteManifestPath,
		manifestIntegrity: `sha512-${createHash("sha512").update(manifestContents).digest("base64")}`,
		packages,
		source: manifest.source,
		getPackage(name) {
			const pkg = packages.find((candidate) => candidate.name === name);
			if (!pkg) throw new Error(`Package is not present in the artifact set: ${name}`);
			return pkg;
		},
	};
}

export function verifyArtifactSet(artifactSet) {
	const verified = readArtifactSet(artifactSet.manifestPath);
	if (verified.manifestIntegrity !== artifactSet.manifestIntegrity) {
		throw new Error(`Package artifact manifest changed after verification: ${artifactSet.manifestPath}`);
	}
	return verified;
}
