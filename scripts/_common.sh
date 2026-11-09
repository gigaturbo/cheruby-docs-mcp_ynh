#!/bin/bash

# Shared steps between install, upgrade and restore.

model_file="embeddinggemma-300M-Q8_0.gguf"

# Settings with no install question. Defaults only: an upgrade keeps whatever
# the config panel has since set.
docs_default_settings() {
    ynh_app_setting_set_default --key=healthcheck_enabled --value=0
    ynh_app_setting_set_default --key=healthcheck_url --value=""
}

# ping.env for the ping unit: the check's URL, or nothing when the feature is
# off. Root 600: anyone holding the URL can ping the check.
docs_write_ping() {
    if [ "$healthcheck_enabled" = "1" ]; then
        echo "HC_URL=$healthcheck_url" > "$install_dir/ping.env"
    else
        echo "HC_URL=" > "$install_dir/ping.env"
    fi
    chown root:root "$install_dir/ping.env"
    chmod 600 "$install_dir/ping.env"
}

# uv is told not to look for configuration files: Yunohost runs scripts from a
# directory under /var/cache/yunohost, and uv would walk up from it.
docs_uv() {
    env HOME="$build_home" UV_CACHE_DIR="$build_home/uv" UV_NO_CONFIG=1 \
        "$install_dir/.uv/uv" "$@"
}

# Code is root-owned and read by the app user through its group: a stolen
# process cannot rewrite what it runs. The venv comes from the hash-locked
# wheels, fetched here as root; the running service downloads nothing.
docs_build_venv() {
    build_home=$(mktemp -d)
    ynh_safe_rm "$install_dir/.uv"
    ynh_safe_rm "$install_dir/venv"
    ynh_setup_source --dest_dir="$install_dir/.uv" --source_id="uv"
    cp "$YNH_APP_BASEDIR/conf/requirements.txt" "$install_dir/requirements.txt"

    pushd "$install_dir"
        ynh_hide_warnings docs_uv venv --python /usr/bin/python3 "$install_dir/venv"
        ynh_hide_warnings docs_uv pip install --python "$install_dir/venv/bin/python" \
            --require-hashes --no-build --requirement "$install_dir/requirements.txt"
    popd

    ynh_safe_rm "$build_home"
    ynh_app_setting_set --key=build_fingerprint --value="$(docs_build_fingerprint)"
}

# What a build depends on. An upgrade that changes neither keeps the venv.
docs_build_fingerprint() {
    { cat "$YNH_APP_BASEDIR/conf/requirements.txt"; ynh_read_manifest "resources.sources.uv.arm64.url"; } \
        | sha256sum | cut -d' ' -f1
}

# llama-server built from the pinned source for this processor, and the model
# beside it. Takes about 8 minutes; the build folder goes once the binary is out.
docs_build_llama() {
    local src
    src=$(mktemp -d)
    ynh_setup_source --dest_dir="$src" --source_id="llama_cpp"
    ynh_hide_warnings cmake -S "$src" -B "$src/build" -DCMAKE_BUILD_TYPE=Release -DGGML_NATIVE=ON \
        -DBUILD_SHARED_LIBS=OFF -DLLAMA_OPENSSL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF
    ynh_hide_warnings nice -n 19 cmake --build "$src/build" --target llama-server -j4
    install -D -m 755 "$src/build/bin/llama-server" "$install_dir/bin/llama-server"
    ynh_safe_rm "$src"

    ynh_safe_rm "$install_dir/model"
    ynh_setup_source --dest_dir="$install_dir/model" --source_id="model"
    ynh_app_setting_set --key=llama_fingerprint --value="$(docs_llama_fingerprint)"
}

# What the binary and the model depend on, the build flags included.
docs_llama_fingerprint() {
    { ynh_read_manifest "resources.sources.llama_cpp.sha256"; ynh_read_manifest "resources.sources.model.sha256"
      declare -f docs_build_llama; } | sha256sum | cut -d' ' -f1
}

# Before an upgrade stops anything: a failed download mid-upgrade makes
# Yunohost remove the app, then restore it.
docs_check_downloads() {
    local url
    for url in https://pypi.org/simple/ https://files.pythonhosted.org/ https://github.com/ \
        "$(ynh_read_manifest "resources.sources.model.url")"; do
        curl --silent --show-error --head --location --output /dev/null --max-time 15 "$url" \
            || ynh_die "$url cannot be reached; nothing was changed. Retry the upgrade later."
    done
}

# The folders setting, one "<user> <Label>=<path>" per line, becomes:
# docs.env (each user's token and folders, root 600), index.env (the folders
# without any token, for the indexer), the units' read-only bind mounts, and
# the groups the units join to read them. A user's token is made once and
# kept in the settings.
docs_configure() {
    local line user rest label path group
    local -A user_folders=()
    local -A groups_seen=()
    bind_paths=""
    groups=""

    while IFS= read -r line; do
        line=$(echo "$line" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
        [ -z "$line" ] && continue
        user=${line%% *}
        rest=${line#* }
        label=${rest%%=*}
        path=${rest#*=}
        [[ "$user" =~ ^[a-z0-9_]+$ ]] || ynh_die "Folders: bad user name in '$line'."
        [[ "$rest" == *=* && "$label" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]] \
            || ynh_die "Folders: '$line' needs <user> <Label>=<path>, the label without spaces or /."
        [[ "$path" == /* && "$path" != *:* && "$path" != *'"'* ]] \
            || ynh_die "Folders: '$path' must be absolute, without : or quotes."
        [ -d "$path" ] || ynh_die "Folders: '$path' is not a folder."
        user_folders[$user]+="${user_folders[$user]:+:}$label=$path"
        bind_paths+=" \"$path\""
        group=$(stat -c %G "$path")
        if [ -z "${groups_seen[$group]:-}" ]; then
            groups_seen[$group]=1
            groups+=" $group"
        fi
    done <<< "$folders"
    [ ${#user_folders[@]} -gt 0 ] || ynh_die "Folders: no folder given."

    # The trace would print the tokens. Each one is first assigned to
    # secret_token, a name Yunohost's log redaction learns the value from.
    {
        echo "DOCS_INDEX_DIR=$data_dir"
        echo "DOCS_EMBED_URL=http://127.0.0.1:$port_embed"
        # Changes with the model file or llama.cpp, either of which changes the vectors.
        echo "DOCS_EMBED_MODEL=$(ynh_read_manifest "resources.sources.model.sha256" | cut -c1-16)-$(ynh_read_manifest "resources.sources.llama_cpp.sha256" | cut -c1-16)"
        for user in "${!user_folders[@]}"; do
            echo "DOCS_FOLDERS_$user=\"${user_folders[$user]}\""
        done
    } > "$install_dir/index.env"

    set +o xtrace
    {
        echo "DOCS_BIND=127.0.0.1"
        echo "DOCS_PORT=$port"
        echo "DOCS_INDEX_DIR=$data_dir"
        echo "DOCS_EMBED_URL=http://127.0.0.1:$port_embed"
        for user in "${!user_folders[@]}"; do
            secret_token=$(ynh_app_setting_get --key="token_$user")
            set -o xtrace
            secret_token=${secret_token:-$(ynh_string_random --length=48)}
            set +o xtrace
            ynh_app_setting_set --key="token_$user" --value="$secret_token"
            echo "DOCS_TOKEN_$user=$secret_token"
            echo "DOCS_FOLDERS_$user=\"${user_folders[$user]}\""
        done
    } > "$install_dir/docs.env"
    set -o xtrace

    local module
    for module in cheruby_docs_mcp.py docs_files.py docs_index.py; do
        install -m 644 "$YNH_APP_BASEDIR/conf/$module" "$install_dir/$module"
    done
    chown -R root:"$app" "$install_dir"
    chmod -R u=rwX,g=rX,o= "$install_dir"
    chown root:root "$install_dir/docs.env" "$install_dir/index.env"
    chmod 600 "$install_dir/docs.env" "$install_dir/index.env"
    chown -R "$app:$app" "$data_dir"
    docs_write_ping

    ynh_config_add_systemd
    ynh_config_add_systemd --service="$app-embed" --template="embed.service"
    # Started by its timer only, so written, not enabled.
    ynh_config_add --template="index.service" --destination="/etc/systemd/system/$app-index.service"
    ynh_config_add --template="index.timer" --destination="/etc/systemd/system/$app-index.timer"
    ynh_config_add --template="ping@.service" --destination="/etc/systemd/system/$app-ping@.service"
    systemctl daemon-reload
    yunohost service add "$app" --description="Cheruby Docs, read-only MCP tools on users' folders"
    yunohost service add "$app-embed" --description="Cheruby Docs, embedding model for the search index"
}

# The embedding server first: the tool server and the indexer call it. It
# answers /health once its model is loaded.
docs_start() {
    ynh_systemctl --service="$app-embed" --action="restart"
    local i
    for i in $(seq 1 60); do
        curl --silent --fail --output /dev/null "http://127.0.0.1:$port_embed/health" && break
        [ "$i" = 60 ] && ynh_die "The embedding server did not answer within two minutes: journalctl -u $app-embed"
        sleep 2
    done
    ynh_systemctl --service="$app" --action="restart" --log_path="systemd" \
        --wait_until="Uvicorn running on" --timeout=60
    systemctl enable --now "$app-index.timer" --quiet
}

# An index left by an earlier install may cover other folders or belong to an
# older user id; the timer rebuilds it.
docs_clear_index() {
    local file
    for file in "$data_dir"/*; do
        [ -e "$file" ] && ynh_safe_rm "$file"
    done
    return 0
}

# Stops everything before files change under it; the timer first, so no index run starts.
docs_stop() {
    systemctl stop "$app-index.timer" "$app-index.service" --quiet 2>/dev/null || true
    ynh_systemctl --service="$app" --action="stop"
    systemctl stop "$app-embed" --quiet 2>/dev/null || true
}
