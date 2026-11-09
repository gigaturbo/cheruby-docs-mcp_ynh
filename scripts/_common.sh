#!/bin/bash

# Shared steps between install, upgrade and restore.

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

# Before an upgrade stops anything: a failed download mid-upgrade makes
# Yunohost remove the app, then restore it.
docs_check_downloads() {
    local url
    for url in https://pypi.org/simple/ https://files.pythonhosted.org/ https://github.com/; do
        curl --silent --show-error --output /dev/null --max-time 15 "$url" \
            || ynh_die "$url cannot be reached; nothing was changed. Retry the upgrade later."
    done
}

# The folders setting, one "<user> <Label>=<path>" per line, becomes:
# docs.env (each user's token and folders, root 600), the unit's read-only
# bind mounts, and the groups the unit joins to read them. A user's token is
# made once and kept in the settings.
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
    set +o xtrace
    {
        echo "DOCS_BIND=127.0.0.1"
        echo "DOCS_PORT=$port"
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

    install -m 644 "$YNH_APP_BASEDIR/conf/cheruby_docs_mcp.py" "$install_dir/cheruby_docs_mcp.py"
    chown -R root:"$app" "$install_dir"
    chmod -R u=rwX,g=rX,o= "$install_dir"
    chown root:root "$install_dir/docs.env"
    chmod 600 "$install_dir/docs.env"

    ynh_config_add_systemd
    yunohost service add "$app" --description="Cheruby Docs, read-only MCP tools on users' folders"
}

docs_start() {
    ynh_systemctl --service="$app" --action="restart" --log_path="systemd" \
        --wait_until="Uvicorn running on" --timeout=60
}
