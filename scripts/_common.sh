#!/bin/bash

# Shared steps between install, upgrade and restore.

model_file="embeddinggemma-300M-Q8_0.gguf"
rerank_model_file="jina-reranker-v2-base-multilingual-Q8_0.gguf"

# Settings with no install question. Defaults only: an upgrade keeps whatever
# the config panel has since set.
docs_default_settings() {
    ynh_app_setting_set_default --key=healthcheck_enabled --value=0
    ynh_app_setting_set_default --key=healthcheck_url --value=""
    # Installs from before the one user: the Whoopy user, or the first user of the folders.
    ynh_app_setting_set_default --key=docs_user \
        --value="${whoopy_user:-$(awk 'NF {print $1; exit}' <<< "${folders:-}")}"
    ynh_app_setting_set_default --key=whoopy_folders --value=""
    ynh_app_setting_set_default --key=index_schedule --value=hourly
    ynh_app_setting_set_default --key=index_calendar --value=""
    ynh_app_setting_set_default --key=index_quiet --value="01-06"
    local key

    # The app password the 0.5 test build made with occ, now pasted in the panel.
    set +o xtrace
    if [ -n "$(ynh_app_setting_get --key=nextcloud_token)" ]; then
        ynh_app_setting_set --key=nextcloud_password --value="$(ynh_app_setting_get --key=nextcloud_token)"
    fi
    set -o xtrace
    ynh_app_setting_delete --key=nextcloud_token
    ynh_app_setting_delete --key=nextcloud_token_user

    for key in syncthing_enabled local_enabled nextcloud_enabled; do
        ynh_app_setting_set_default --key="$key" --value=0
    done
    for key in syncthing_folders syncthing_only syncthing_skip syncthing_writable local_folders local_only \
        local_skip local_writable nextcloud_account nextcloud_folders nextcloud_only nextcloud_skip nextcloud_writable; do
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
            while IFS=$'\t' read -r sync_label sync_path ignore _; do
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

# Syncthing's folders, one "<label>\t<path>\t<ignorePerms>\t<type>" per line,
# type such as sendreceive or receiveonly, read from its config file where the
# YunoHost Syncthing package keeps it. Nothing when Syncthing is not installed.
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
        print(label, path, folder.get("ignorePerms", "false"), folder.get("type") or "sendreceive", sep="\t")
' "$file"
        return 0
    done
}

# Where this server's Nextcloud is, from its package's settings: nc_host,
# nc_account (nextcloud_account, or the app's user) and nc_url, that
# account's WebDAV root as the units reach it, on loopback. Returns 1 when
# Nextcloud is not installed.
docs_nextcloud_where() {
    [ -f /etc/yunohost/apps/nextcloud/settings.yml ] || return 1
    nc_host=$(ynh_app_setting_get --app=nextcloud --key=domain)
    nc_account=${nextcloud_account:-$docs_user}
    nc_url="https://127.0.0.1$(ynh_app_setting_get --app=nextcloud --key=path | sed 's|/*$||')/remote.php/dav/files/$(jq -rn --arg a "$nc_account" '$a | @uri')"
}

# "<code> <permissions>" for an app password on a folder of the account: the
# HTTP code, 207 when it can be read, 000 when Nextcloud does not answer, and
# Nextcloud's permission letters for the account there, C and K when it may
# add files and folders. Needs the trace off: the password goes to curl on
# stdin, never on its command line.
docs_nextcloud_code() { # <path in the account> <password>
    local quoted answer
    quoted=$(jq -rn --arg p "$1" '$p | split("/") | map(select(length > 0) | @uri) | join("/")')
    answer=$(curl --silent --write-out '\n%{http_code}' --max-time 20 --config - \
        --resolve "$nc_host:443:127.0.0.1" --request PROPFIND --header "Depth: 0" \
        --header "Content-Type: application/xml" \
        --data '<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns"><d:prop><oc:permissions/></d:prop></d:propfind>' \
        "https://$nc_host${nc_url#https://127.0.0.1}/$quoted" \
        <<< "user = \"$(sed 's/["\\]/\\&/g' <<< "$nc_account"):$2\"" || true)
    echo "${answer##*$'\n'} $(grep -o 'permissions>[A-Z]*<' <<< "$answer" | head -n 1 | tr -dc 'A-Z')"
}

# The tool label of a Syncthing folder: "Android Pictures" is Android_Pictures.
docs_label() {
    LC_ALL=C sed 's/[^A-Za-z0-9_.-]/_/g; s/^[^A-Za-z0-9]*//' <<< "$1"
}

# The folders of the turned-on sources, from the settings' variables, into
# folder_path[label] and folder_source[label], and folder_writable[label] for
# those marked writable. A bad entry is left out and described in
# folder_error, one line each, a bad writable mark in write_error: the panel
# refuses to save them, while an upgrade or a restore, which cannot ask, goes
# on without them.
docs_collect_folders() {
    declare -gA folder_path=() folder_source=() folder_writable=()
    folder_error=""
    write_error=""
    # The same labels from the web admin and from a shell in another locale.
    local LC_ALL=C
    local candidates="" sync_label path ignore kind line label source wanted key
    local -A names=([syncthing]="Syncthing" [local]="Local storage" [nextcloud]="Nextcloud") kinds=()
    if [ "${syncthing_enabled:-0}" = 1 ]; then
        local -A synced=()
        while IFS=$'\t' read -r sync_label path ignore kind; do
            synced[$sync_label]=$path
            kinds[$(docs_label "$sync_label")]=$kind
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
    if [ "${nextcloud_enabled:-0}" = 1 ] && ! docs_nextcloud_where; then
        folder_error+="Nextcloud: it is not installed."$'\n'
    elif [ "${nextcloud_enabled:-0}" = 1 ]; then
        # Empty opens the whole account.
        while IFS= read -r line; do
            line=$(sed 's/^[[:space:]]*//; s/[[:space:]]*$//' <<< "$line")
            [[ -z "$line" || "$line" == \#* ]] && continue
            if [[ "$line" != *=* ]]; then
                folder_error+="Nextcloud: '$line' needs <Label>=<folder in the account>."$'\n'
                continue
            fi
            candidates+="nextcloud"$'\t'"${line%%=*}"$'\t'"nextcloud:/$(sed 's|^/*||' <<< "${line#*=}")"$'\n'
        done <<< "${nextcloud_folders:-Nextcloud=/}"
    fi
    while IFS=$'\t' read -r source label path; do
        [ -z "$source" ] && continue
        if ! [[ "$label" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]]; then
            folder_error+="${names[$source]}: '$label' is not a label: letters, digits, _ . - and no space."$'\n'
        elif [ -n "${folder_path[$label]+x}" ]; then
            folder_error+="${names[$source]}: the label $label is already a ${names[${folder_source[$label]}]} folder."$'\n'
        elif [ "$source" = nextcloud ] && [[ "${path#nextcloud:}/" == */.* || "$path" == *'"'* ]]; then
            folder_error+="Nextcloud: '${path#nextcloud:}' must be a folder of the account, without hidden names, .. or quotes."$'\n'
        elif [ "$source" = nextcloud ]; then
            folder_path[$label]=$(sed 's|//*|/|g' <<< "$path")
            folder_source[$label]=$source
        elif ! [[ "$path" == /* && "$path" != *:* && "$path" != *'"'* ]]; then
            folder_error+="${names[$source]}: '$path' must be absolute, without : or quotes."$'\n'
        elif [ ! -d "$path" ]; then
            folder_error+="${names[$source]}: '$path' is not a folder."$'\n'
        else
            folder_path[$label]=$path
            folder_source[$label]=$source
        fi
    done <<< "$candidates"

    # Labels as the tools show them. Syncthing reverts what is written in a
    # receive-only folder; the multimedia and backup shares stay read-only.
    local -a marked=()
    for source in syncthing local nextcloud; do
        key=${source}_enabled
        [ "${!key:-0}" = 1 ] || continue
        key=${source}_writable
        IFS=, read -ra marked <<< "${!key:-}"
        for label in "${marked[@]}"; do
            if [ "${folder_source[$label]:-}" != "$source" ]; then
                write_error+="${names[$source]}: no folder $label to make writable."$'\n'
            elif [ "$source" = syncthing ] && [ "${kinds[$label]:-}" = receiveonly ]; then
                write_error+="Syncthing: $label is receive-only, and Syncthing would revert what is written there."$'\n'
            elif [[ "${folder_path[$label]}/" == /home/yunohost.multimedia/* || "${folder_path[$label]}/" == /home/yunohost.backup/* ]]; then
                write_error+="${names[$source]}: $label is in a YunoHost multimedia or backup share, which stays read-only."$'\n'
            else
                folder_writable[$label]=1
            fi
        done
    done
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

# llama-server built from the pinned source for this processor, and the
# embedding and reranking models beside it. Takes about 8 minutes; the build folder goes once the binary is out.
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
    ynh_setup_source --dest_dir="$install_dir/model" --source_id="rerank_model"
    ynh_app_setting_set --key=llama_fingerprint --value="$(docs_llama_fingerprint)"
}

# What the binary and the model depend on, the build flags included.
docs_llama_fingerprint() {
    { ynh_read_manifest "resources.sources.llama_cpp.sha256"; ynh_read_manifest "resources.sources.model.sha256"
      ynh_read_manifest "resources.sources.rerank_model.sha256"
      declare -f docs_build_llama; } | sha256sum | cut -d' ' -f1
}

# Before an upgrade stops anything: a failed download mid-upgrade makes
# Yunohost remove the app, then restore it.
docs_check_downloads() {
    local url
    for url in https://pypi.org/simple/ https://files.pythonhosted.org/ https://github.com/ \
        "$(ynh_read_manifest "resources.sources.model.url")" \
        "$(ynh_read_manifest "resources.sources.rerank_model.url")"; do
        curl --silent --show-error --head --location --output /dev/null --max-time 15 "$url" \
            || ynh_die "$url cannot be reached; nothing was changed. Retry the upgrade later."
    done
}

# The sources' settings become: folders.json (the user's folders and their
# filters), index.env (the Nextcloud app password, no MCP token) and docs.env
# (the tokens), both root 600, the units' read-only bind mounts, and the
# groups the units join to read the folders on disk. Each token is made once
# and kept in the settings.
docs_configure() {
    local label path group token only skip line nextcloud_env="" nc_ready=0
    local -A groups_seen=()
    # Every folder on disk for the indexer; read-only or writable for the tool server.
    bind_paths=""
    read_paths=""
    write_paths=""
    groups=""
    docs_collect_folders
    while IFS= read -r line; do
        [ -n "$line" ] && ynh_print_warn "$line Left out."
    done <<< "$folder_error"
    while IFS= read -r line; do
        [ -n "$line" ] && ynh_print_warn "$line It stays read-only."
    done <<< "$write_error"

    # The app password is pasted in the panel; without it, the Nextcloud
    # folders are left out. Only the nc_ready flag leaves the untraced part.
    set +o xtrace
    if printf '%s\n' "${folder_source[@]}" | grep -qx nextcloud; then
        token=$(ynh_app_setting_get --key=nextcloud_password)
        if [ -n "$token" ]; then
            nextcloud_env="DOCS_NEXTCLOUD_URL=$nc_url"$'\n'"DOCS_NEXTCLOUD_HOST=$nc_host"
            nextcloud_env+=$'\n'"DOCS_NEXTCLOUD_USER=$nc_account"$'\n'"DOCS_NEXTCLOUD_PASSWORD=$token"
            nc_ready=1
        fi
        token=""
    fi
    set -o xtrace
    if [ "$nc_ready" = 0 ]; then
        for label in "${!folder_source[@]}"; do
            if [ "${folder_source[$label]}" = nextcloud ]; then
                ynh_print_warn "Nextcloud: no app password is saved in the config panel; $label is left out."
                unset "folder_path[$label]" "folder_source[$label]"
            fi
        done
    fi
    [ ${#folder_path[@]} -gt 0 ] || ynh_print_warn "No folder is open: turn on a source in the config panel."

    # Tags settings hold their values joined by ",".
    local entries="{}"
    for label in "${!folder_path[@]}"; do
        path=${folder_path[$label]}
        only=${folder_source[$label]}_only
        skip=${folder_source[$label]}_skip
        entries=$(jq -c --arg l "$label" --arg p "$path" --arg only "${!only:-}" --arg skip "${!skip:-}" \
            --arg w "${folder_writable[$label]:-}" \
            '.[$l] = (if $p | startswith("nextcloud:") then {nextcloud: ($p | ltrimstr("nextcloud:"))} else {path: $p} end)
                     + {only: ($only | split(",") | map(select(length > 0))),
                        skip: ($skip | split(",") | map(select(length > 0)))}
                     + (if $w == "1" then {writable: true} else {} end)' <<< "$entries")
        # Nextcloud's folders are read through Nextcloud, not on disk.
        [ "${folder_source[$label]}" = nextcloud ] && continue
        bind_paths+=" \"$path\""
        if [ -n "${folder_writable[$label]:-}" ]; then
            write_paths+=" \"$path\""
        else
            read_paths+=" \"$path\""
        fi
        group=$(stat -c %G "$path")
        if [ -z "${groups_seen[$group]:-}" ]; then
            groups_seen[$group]=1
            groups+=" $group"
        fi
    done
    jq --arg u "$docs_user" '{($u): .}' <<< "$entries" > "$install_dir/folders.json"
    # A leading space would reach the units as an empty path, which systemd warns about.
    bind_paths=${bind_paths# }
    read_paths=${read_paths# }
    write_paths=${write_paths# }

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

    # Changes with the model file or llama.cpp, either of which changes the vectors.
    local model
    model="$(ynh_read_manifest "resources.sources.model.sha256" | cut -c1-16)-$(ynh_read_manifest "resources.sources.llama_cpp.sha256" | cut -c1-16)"

    # The trace stays off while a token is in a variable: it would go to the
    # operation log and, unredacted, to yunohost-cli.log.
    set +o xtrace
    {
        echo "DOCS_FOLDERS_FILE=$install_dir/folders.json"
        echo "DOCS_INDEX_DIR=$data_dir"
        echo "DOCS_EMBED_URL=http://127.0.0.1:$port_embed"
        echo "DOCS_EMBED_MODEL=$model"
        echo "DOCS_QUIET_HOURS=${index_quiet:-}"
        [ -z "$nextcloud_env" ] || echo "$nextcloud_env"
    } > "$install_dir/index.env"
    {
        echo "DOCS_BIND=127.0.0.1"
        echo "DOCS_PORT=$port"
        echo "DOCS_FOLDERS_FILE=$install_dir/folders.json"
        echo "DOCS_INDEX_DIR=$data_dir"
        echo "DOCS_EMBED_URL=http://127.0.0.1:$port_embed"
        echo "DOCS_RERANK_URL=http://127.0.0.1:$port_rerank"
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
        [ -z "$nextcloud_env" ] || echo "$nextcloud_env"
        token=""
    } > "$install_dir/docs.env"
    nextcloud_env=""
    set -o xtrace

    local module
    for module in cheruby_docs_mcp.py docs_files.py docs_index.py docs_text.py; do
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
    ynh_config_add_systemd --service="$app-rerank" --template="rerank.service"
    # Started by its timer only, so written, not enabled.
    ynh_config_add --template="index.service" --destination="/etc/systemd/system/$app-index.service"
    docs_write_timer
    ynh_config_add --template="ping@.service" --destination="/etc/systemd/system/$app-ping@.service"
    systemctl daemon-reload
    yunohost service add "$app" --description="Cheruby Docs, MCP tools on users' folders"
    yunohost service add "$app-embed" --description="Cheruby Docs, embedding model for the search index"
    yunohost service add "$app-rerank" --description="Cheruby Docs, reranking model for search results"
}

# The embedding server first: the tool server and the indexer call it. It
# answers /health once its model is loaded. Search goes on without the
# reranking server, so nothing waits for it.
docs_start() {
    ynh_systemctl --service="$app-rerank" --action="restart"
    ynh_systemctl --service="$app-embed" --action="restart"
    local i
    for i in $(seq 1 60); do
        curl --silent --fail --output /dev/null "http://127.0.0.1:$port_embed/health" && break
        [ "$i" = 60 ] && ynh_die "The embedding server did not answer within two minutes: journalctl -u $app-embed"
        sleep 2
    done
    # Started beside the embedding server, but it may load a little later.
    for i in $(seq 1 10); do
        curl --silent --fail --output /dev/null "http://127.0.0.1:$port_rerank/health" && break
        [ "$i" = 10 ] && ynh_print_warn "The reranking server does not answer yet; search runs in a rougher order until it does: journalctl -u $app-rerank"
        sleep 2
    done
    ynh_systemctl --service="$app" --action="restart" --log_path="systemd" \
        --wait_until="Uvicorn running on" --timeout=60
    docs_start_timer
}

# The index timer at the schedule chosen in the panel, never in its quiet
# hours, by default 01 to 06, around the nightly backups (01:30 to about 05:45).
docs_write_timer() {
    on_calendar=$(docs_calendar "$(docs_index_cron)" "${index_quiet:-}")
    if [ -z "$on_calendar" ]; then
        ynh_print_warn "Search index: the schedule '$(docs_index_cron)' with quiet hours '${index_quiet:-}' leaves no time to run; the index runs hourly."
        on_calendar=$(docs_calendar "20 * * * *" "${index_quiet:-}")
        on_calendar=${on_calendar:-*:20}
    fi
    ynh_config_add --template="index.timer" --destination="/etc/systemd/system/$app-index.timer"
}

# The cron schedule of the panel's choice: a preset, or the custom one.
docs_index_cron() {
    local -A preset=([hourly]="20 * * * *" [6h]="20 */6 * * *" [nightly]="20 23 * * *")
    if [ "${index_schedule:-hourly}" = custom ]; then
        echo "${index_calendar:-}"
    else
        echo "${preset[${index_schedule:-hourly}]:-20 * * * *}"
    fi
}

# The OnCalendar of a cron schedule (minute hour day month weekday), its
# minutes and hours listed one by one so the quiet hours (<from>-<to>, to
# excluded, empty for none) can be left out. Prints nothing when cron's
# meaning cannot be kept, no hour is left, or systemd does not take it.
docs_calendar() { # <cron> <quiet hours>
    local expr
    expr=$(python3 -I - "$1" "$2" <<'EOF'
import re
import sys

fields, quiet = sys.argv[1].lower().split(), sys.argv[2]
if len(fields) != 5:
    sys.exit(1)
NAMES = {**{d: str(n) for n, d in enumerate(["sun", "mon", "tue", "wed", "thu", "fri", "sat"])},
         **{m: str(n) for n, m in enumerate(["jan", "feb", "mar", "apr", "may", "jun",
                                             "jul", "aug", "sep", "oct", "nov", "dec"], 1)}}
DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]


def values(field, low, high):
    """The sorted numbers a cron field names; exits on what it cannot read."""
    out = set()
    for part in re.sub(r"[a-z]{3}", lambda m: NAMES.get(m[0], m[0]), field).split(","):
        m = re.fullmatch(r"(\*|\d+)(?:-(\d+))?(?:/(\d+))?", part)
        if not m or (m[1] == "*" and m[2]):
            sys.exit(1)
        start = low if m[1] == "*" else int(m[1])
        end = int(m[2]) if m[2] else high if m[1] == "*" or m[3] else start
        if not low <= start <= end <= high or (m[3] and int(m[3]) == 0):
            sys.exit(1)
        out.update(range(start, end + 1, int(m[3] or 1)))
    return sorted(out)


minutes, hours, days, months, weekdays = (values(f, lo, hi) for f, lo, hi in
                                          zip(fields, (0, 0, 1, 1, 0), (59, 23, 31, 12, 7), strict=True))
# cron runs when either the day or the weekday matches, systemd when both do.
if fields[2] != "*" and fields[4] != "*":
    sys.exit(1)
if quiet:
    a, b = (int(h) % 24 for h in quiet.split("-"))
    if a == b:
        sys.exit(1)
    hours = [h for h in hours if not (a <= h < b if a < b else h >= a or h < b)]
if not hours:
    sys.exit(1)
listed = lambda found, every: "*" if found == every else ",".join(map(str, found))
weekday = "" if fields[4] == "*" else ",".join(dict.fromkeys(DAYS[d] for d in weekdays)) + " "
print(f"{weekday}*-{listed(months, list(range(1, 13)))}-{listed(days, list(range(1, 32)))} "
      f"{listed(hours, list(range(24)))}:{listed(minutes, list(range(60)))}:00")
EOF
) || return 0
    systemd-analyze calendar "$expr" > /dev/null 2>&1 && echo "$expr" || true
}

# Started, or stopped when the schedule is never. A restart applies a new schedule.
docs_start_timer() {
    if [ "${index_schedule:-hourly}" = never ]; then
        systemctl disable --now "$app-index.timer" --quiet
    else
        systemctl enable "$app-index.timer" --quiet
        systemctl restart "$app-index.timer"
    fi
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
    systemctl stop "$app-embed" "$app-rerank" --quiet 2>/dev/null || true
}

# The app's logo, shown by the webadmin and the portal, set only while it has
# none, so a logo chosen in the webadmin survives upgrades. PNG only.
docs_set_logo() {
    yunohost tools shell -c "
from yunohost.permission import user_permission_info
from yunohost.user import user_permission_update as update_tile
if not user_permission_info('$app.main').get('logo_hash'):
    update_tile('$app.main', logo=open('$YNH_APP_BASEDIR/conf/logo.png', 'rb'))
"
}
