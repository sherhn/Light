from common import match, module, outline, region, words, npm_server

INSTRUCTIONS = "FROM RUN CMD LABEL EXPOSE ENV ADD COPY ENTRYPOINT VOLUME USER WORKDIR ARG ONBUILD STOPSIGNAL HEALTHCHECK SHELL MAINTAINER AS".split()
YAML = [match(r"(?:^|[ \t])(#.*)$", captures={1: "comment"}), match(r'"(?:[^"\\\n]|\\.)*"?', "string"), match(r"'[^'\n]*'?", "string"), match(r"\$\{[^}\n]+\}", "variable"), match(r"^[ \t]*(?:-[ \t]*)?([\w.-]+)[ \t]*:", captures={1: "keyword"}), words("true false null yes no on off".split(), "constant", ignore_case=True), match(r"\b\d+(?:\.\d+)?\b", "number")]
COMPOSE = ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml", "compose.*.yaml", "compose.*.yml", "docker-compose.*.yml", "docker-compose.*.yaml"]

MODULE = module(
    version="1.1.0",
    id="docker", name="Docker", category="Tools",
    summary="Dockerfile and Compose: syntax, completion, stages/services outline, build checks, language servers, images, containers and Compose lifecycle commands.",
    description="Dockerfile instructions, variables, flags and multi-stage builds; Compose YAML services, environment variables, indentation and folding; dockerignore patterns. Build and validate Dockerfiles, list containers/images, inspect daemon status; Compose validate, build, pull, up, stop, restart, logs, ps and down (without deleting volumes). Commands run through the configured local or WSL environment and stream to the Run panel. Requires an existing Docker Engine or Docker Desktop with Compose v2; Linux tools can be installed from Settings. No daemon, image, container or volume is changed until you explicitly run an action.",
    languages=[
        dict(id="dockerfile", name="Dockerfile", extensions=["dockerfile"], filenames=["Dockerfile", "Containerfile", "Dockerfile.*", "Containerfile.*"], line_comment="#", brackets=["[]", "{}"], indent={"mode": "keep", "size": 4}, fold="indent", grammar=[match(r"^[ \t]*#.*$", "comment"), match(r'"(?:[^"\\\n]|\\.)*"?', "string"), match(r"'[^'\n]*'?", "string"), match(r"\$\{[^}\n]+\}|\$[A-Za-z_]\w*", "variable"), match(r"--[a-z][\w-]*(?:=[^ \t\n]+)?", "argument"), words(INSTRUCTIONS, "keyword", ignore_case=True), match(r"\b\d+(?:/\w+)?\b", "number")], outline=[outline(r"(?i)^[ \t]*FROM[ \t]+[^\n]+[ \t]+AS[ \t]+([\w.-]+)", "module")]),
        dict(id="docker-compose", name="Docker Compose", filenames=COMPOSE, line_comment="#", brackets=["[]", "{}"], indent={"mode": "python", "size": 2}, fold="indent", grammar=YAML, outline=[outline(r"^  ([A-Za-z_][\w.-]*)[ \t]*:", "module")]),
        dict(id="dockerignore", name="Docker ignore", filenames=[".dockerignore", "*.dockerignore"], line_comment="#", indent={"mode": "keep", "size": 2}, grammar=[match(r"^[ \t]*#.*$", "comment"), match(r"^!.*$", "keyword"), match(r"[*?]+", "constant")]),
    ],
    run=[dict(language="dockerfile", name="Docker: Build image", command='docker build -f "{file}" -t "{image}" "{dir}"'), dict(language="dockerfile", name="Docker: Validate build", command='docker build --check -f "{file}" "{dir}"')]
        + [dict(language="dockerfile", name="Docker: " + label, command=command) for label, command in [("Run project container", 'docker run -d --name "{image}" "{image}"'), ("Start project container", 'docker start "{image}"'), ("Stop project container", 'docker stop "{image}"'), ("Restart project container", 'docker restart "{image}"'), ("Project container logs", 'docker logs --tail 200 "{image}"'), ("Inspect project container", 'docker inspect "{image}"'), ("Remove stopped project container", 'docker rm "{image}"')]]
        + [dict(language=language, name="Docker: " + label, command=command) for language in ["dockerfile", "docker-compose"] for label, command in [("Containers", "docker ps -a"), ("Images", "docker image ls"), ("Engine status", "docker info")]]
        + [dict(language="docker-compose", name="Compose: " + label, command='docker compose -f "{file}" ' + command) for label, command in [("Validate", "config --quiet"), ("Build", "build"), ("Pull", "pull"), ("Start services", "up -d"), ("Stop services", "stop"), ("Restart services", "restart"), ("Service status", "ps --all"), ("Logs", "logs --no-color --tail 200"), ("Remove containers (keep volumes)", "down")]],
    check=[dict(language="docker-compose", name="Compose validation", command='docker compose -f "{file}" config --quiet', pattern=r"(?m)^(?:(?:yaml: )?line (?P<line>\d+):[ \t]*)?(?P<msg>[^\n]+)")],
    # the Dockerfile server of rcjsuen and Microsoft's Compose server (both MIT)
    lsp=[npm_server("dockerfile-language-server", ["dockerfile"], "docker-langserver --stdio", "0.15.0", 4_000_000),
         npm_server("compose-language-service", ["docker-compose"], "docker-compose-langserver --stdio", "1.0.0", 5_000_000,
                    language_ids={"docker-compose": "dockercompose"})],
    tools=[dict(name="Docker Engine and Compose", programs=["docker"], apt="docker.io docker-compose-v2")],
    detect=dict(extensions=["dockerfile"], files=["Dockerfile", "Containerfile", "Dockerfile.*", "Containerfile.*", ".dockerignore", "*.dockerignore"] + COMPOSE),
)
