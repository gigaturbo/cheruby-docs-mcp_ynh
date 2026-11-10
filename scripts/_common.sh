#!/bin/bash

# Shared steps between install, upgrade and restore.

model_file="embeddinggemma-300M-Q8_0.gguf"

# Settings with no install question. Defaults only: an upgrade keeps whatever
# the config panel has since set.
docs_default_settings() {
    ynh_app_setting_set_default --key=healthcheck_enabled --value=0
    ynh_app_setting_set_default --key=healthcheck_url --value=""
    # Installs from before the one user: the Whoopy user, or the first user of the folders.
    ynh_app_setting_set_default --key=docs_user \
        --value="${whoopy_user:-$(awk 'NF {print $1; exit}' <<< "${folders:-}")}"
    ynh_app_setting_set_default --key=whoopy_folders --value=""
    local key
    for key in syncthing_enabled local_enabled; do
        ynh_app_setting_set_default --key="$key" --value=0
    done
    for key in syncthing_folders syncthing_only syncthing_skip local_folders local_only local_skip; do
        ynh_app_setting_set_default --key="$key" --value=""
    done

    # Installs from before the sources: each of the user's folders becomes the
    # Syncthing folder on that path and with that label, or a local folder.
    if [ -n "${folders:-}" ]; then
        local line user rest label path sync_label sync_path ignore found syncs="" locals=""
        while IFS= read -r line; do
            read -r user rest <<< "$line"
            [ -z "$user" ] && continue
            label=${rest%%=*}
            path=${rest#*=}
            if [ "$user" != "$docs_user" ]; then
                ynh_print_warn "Folders: $label of $user is left out: the app now serves $docs_user only."
                ynh_app_setting_delete --key="token_$user"
                continue
            fi
            found=""
            while IFS=$'\t' read -r sync_label sync_path ignore; do
                if [ "$sync_path" = "$path" ] && [ "$(docs_label "$sync_label")" = "$label" ]; then
                    found=$sync_label
                fi
            done < <(docs_syncthing_folders)
            if [ -n "$found" ]; then
                syncs+="${syncs:+,}$found"
            else
                locals+="${locals:+$'\n'}$label=$path"
            fi
        done <<< "$folders"
        if [ -n "$syncs" ]; then
            syncthing_enabled=1
            syncthing_folders=$syncs
        fi
        if [ -n "$locals" ]; then
            local_enabled=1
            local_folders=$locals
        fi
        for key in syncthing_enabled syncthing_folders local_enabled local_folders; do
            ynh_app_setting_set --key="$key" --value="${!key}"
        done
        ynh_app_setting_delete --key=folders
        ynh_app_setting_delete --key=whoopy_user
    fi
}

# Syncthing's folders, one "<label>\t<path>\t<ignorePerms>" per line, read from
# its config file where the YunoHost Syncthing package keeps it. Nothing when
# Syncthing is not installed.
docs_syncthing_folders() {
    local dir file
    [ -f /etc/yunohost/apps/syncthing/settings.yml ] || return 0
    dir=$(ynh_app_setting_get --app=syncthing --key=install_dir)
    for file in "$dir/.config/syncthing/config.xml" "$dir/.local/state/syncthing/config.xml"; do
        [ -f "$file" ] || continue
        # Top-level folders only: the one under <defaults> is a template.
        python3 -I -c '
import sys
import xml.etree.ElementTree as ET
try:
    root = ET.parse(sys.argv[1]).getroot()
except (OSError, ET.ParseError):
    sys.exit()
for folder in root.findall("folder"):
    label, path = folder.get("label") or folder.get("id") or "", folder.get("path") or ""
    if label and path and not any(c in label + path for c in "\t\n,"):
        print(label, path, folder.get("ignorePerms", "false"), sep="\t")
' "$file"
        return 0
    done
}

# The tool label of a Syncthing folder: "Android Pictures" is Android_Pictures.
docs_label() {
    LC_ALL=C sed 's/[^A-Za-z0-9_.-]/_/g; s/^[^A-Za-z0-9]*//' <<< "$1"
}

# The folders of the turned-on sources, from the settings' variables, into
# folder_path[label] and folder_source[label]. A bad entry is left out and
# described in folder_error, one line each: the panel refuses to save it,
# while an upgrade or a restore, which cannot ask, goes on without it.
docs_collect_folders() {
    declare -gA folder_path=() folder_source=()
    folder_error=""
    # The same labels from the web admin and from a shell in another locale.
    local LC_ALL=C
    local candidates="" sync_label path ignore line label source wanted
    local -A names=([syncthing]="Syncthing" [local]="Local storage")
    if [ "${syncthing_enabled:-0}" = 1 ]; then
        local -A synced=()
        while IFS=$'\t' read -r sync_label path ignore; do
            synced[$sync_label]=$path
        done < <(docs_syncthing_folders)
        [ ${#synced[@]} -gt 0 ] || folder_error+="Syncthing: no folder found. Is Syncthing installed?"$'\n'
        local -a picked=()
        IFS=, read -ra picked <<< "${syncthing_folders:-}"
        [ ${#picked[@]} -gt 0 ] || picked=("${!synced[@]}")
        for wanted in "${picked[@]}"; do
            if [ -z "${synced[$wanted]+x}" ]; then
                folder_error+="Syncthing has no folder '$wanted'."$'\n'
                continue
            fi
            candidates+="syncthing"$'\t'"$(docs_label "$wanted")"$'\t'"${synced[$wanted]}"$'\n'
        done
    fi
    if [ "${local_enabled:-0}" = 1 ]; then
        while IFS= read -r line; do
            line=$(sed 's/^[[:space:]]*//; s/[[:space:]]*$//' <<< "$line")
            [[ -z "$line" || "$line" == \#* ]] && continue
            if [[ "$line" != *=* ]]; then
                folder_error+="Local storage: '$line' needs <Label>=<path>."$'\n'
                continue
            fi
            candidates+="local"$'\t'"${line%%=*}"$'\t'"${line#*=}"$'\n'
        done <<< "${local_folders:-}"
    fi
    while IFS=$'\t' read -r source label path; do
        [ -z "$source" ] && continue
        if ! [[ "$label" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]]; then
            folder_error+="${names[$source]}: '$label' is not a label: letters, digits, _ . - and no space."$'\n'
        elif [ -n "${folder_path[$label]+x}" ]; then
            folder_error+="${names[$source]}: the label $label is already a ${names[${folder_source[$label]}]} folder."$'\n'
        elif ! [[ "$path" == /* && "$path" != *:* && "$path" != *'"'* ]]; then
            folder_error+="${names[$source]}: '$path' must be absolute, without : or quotes."$'\n'
        elif [ ! -d "$path" ]; then
            folder_error+="${names[$source]}: '$path' is not a folder."$'\n'
        else
            folder_path[$label]=$path
            folder_source[$label]=$source
        fi
    done <<< "$candidates"
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

# The sources' settings become: folders.json (the user's folders and their
# filters), index.env (no token) and docs.env (the tokens), both root 600,
# the units' read-only bind mounts, and the groups the units join to read
# the folders. Each token is made once and kept in the settings.
docs_configure() {
    local label path group token only skip line
    local -A groups_seen=()
    bind_paths=""
    groups=""
    docs_collect_folders
    while IFS= read -r line; do
        [ -n "$line" ] && ynh_print_warn "$line Left out."
    done <<< "$folder_error"
    [ ${#folder_path[@]} -gt 0 ] || ynh_print_warn "No folder is open: turn on a source in the config panel."

    # Tags settings hold their values joined by ",".
    local entries="{}"
    for label in "${!folder_path[@]}"; do
        path=${folder_path[$label]}
        only=${folder_source[$label]}_only
        skip=${folder_source[$label]}_skip
        entries=$(jq -c --arg l "$label" --arg p "$path" --arg only "${!only:-}" --arg skip "${!skip:-}" \
            '.[$l] = {path: $p, only: ($only | split(",") | map(select(length > 0))),
                      skip: ($skip | split(",") | map(select(length > 0)))}' <<< "$entries")
        bind_paths+=" \"$path\""
        group=$(stat -c %G "$path")
        if [ -z "${groups_seen[$group]:-}" ]; then
            groups_seen[$group]=1
            groups+=" $group"
        fi
    done
    jq --arg u "$docs_user" '{($u): .}' <<< "$entries" > "$install_dir/folders.json"

    # The Whoopy token opens whoopy_folders (labels joined by ","), or all the
    # folders when empty. A label no longer among them is dropped.
    local whoopy_labels="" wanted
    local -a picked=()
    IFS=, read -ra picked <<< "${whoopy_folders:-}"
    for wanted in "${picked[@]}"; do
        if [ -n "${folder_path[$wanted]+x}" ]; then
            whoopy_labels+="${whoopy_labels:+:}$wanted"
        else
            ynh_print_warn "Whoopy access: no folder '$wanted' any more; left out."
        fi
    done
    if [ ${#picked[@]} -eq 0 ]; then
        whoopy_labels=$(printf '%s\n' "${!folder_path[@]}" | sed '/^$/d' | sort | paste -sd:)
    fi
    [ -n "$whoopy_labels" ] || ynh_print_warn "Whoopy access: the token opens no folder."

    {
        echo "DOCS_FOLDERS_FILE=$install_dir/folders.json"
        echo "DOCS_INDEX_DIR=$data_dir"
        echo "DOCS_EMBED_URL=http://127.0.0.1:$port_embed"
        # Changes with the model file or llama.cpp, either of which changes the vectors.
        echo "DOCS_EMBED_MODEL=$(ynh_read_manifest "resources.sources.model.sha256" | cut -c1-16)-$(ynh_read_manifest "resources.sources.llama_cpp.sha256" | cut -c1-16)"
    } > "$install_dir/index.env"

    # The trace stays off while a token is in a variable: it would go to the
    # operation log and, unredacted, to yunohost-cli.log.
    set +o xtrace
    {
        echo "DOCS_BIND=127.0.0.1"
        echo "DOCS_PORT=$port"
        echo "DOCS_FOLDERS_FILE=$install_dir/folders.json"
        echo "DOCS_INDEX_DIR=$data_dir"
        echo "DOCS_EMBED_URL=http://127.0.0.1:$port_embed"
        token=$(ynh_app_setting_get --key="token_$docs_user")
        token=${token:-$(ynh_string_random --length=48)}
        ynh_app_setting_set --key="token_$docs_user" --value="$token"
        echo "DOCS_TOKEN_$docs_user=$token"
        token=$(ynh_app_setting_get --key=whoopy_token)
        token=${token:-$(ynh_string_random --length=48)}
        ynh_app_setting_set --key=whoopy_token --value="$token"
        echo "DOCS_GATEWAY_TOKEN=$token"
        echo "DOCS_GATEWAY_USER=$docs_user"
        echo "DOCS_GATEWAY_FOLDERS=$whoopy_labels"
        token=""
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
