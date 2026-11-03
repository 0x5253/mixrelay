-- WirePlumber 0.5 policy hook for MixRelay-owned render endpoints only.
-- The engine confirms pre-mute capture and an exact process/stream/endpoint lease
-- before submitting it. This script does not change defaults or other targets.

Script.async_activation = true

local lutils = require ("linking-utils")
local log = Log.open_topic ("s-mixrelay-source-mute")
log:info ("source-mute script entered")
local nodes = ObjectManager { Interest { type = "node" } }
local clients = ObjectManager { Interest { type = "client" } }
nodes:activate ()
clients:activate ()

local leases = {}
-- The script environment keeps the exported metadata alive until script shutdown.
metadata = ImplMetadata ("mixrelay-source-mute")

local function rescan_linking ()
  local source = Plugin.find ("standard-event-source")
  if source then source:call ("schedule-rescan", "linking") end
end

local function verified_client (node)
  -- Node application.* properties are client assertions. The PipeWire server
  -- supplies this Client's security PID/UID and its registry-bound identity.
  local client_id = node.properties ["client.id"]
  if not client_id then return nil end
  for client in clients:iterate () do
    if tostring (client ["bound-id"]) == tostring (client_id) then
      local props = client.properties
      if not props ["object.serial"] then return nil end
      return props ["pipewire.sec.pid"], props ["pipewire.sec.uid"], props
    end
  end
  return nil
end

local function source_owner_matches (source, app_pid, owner_pid)
  local actual_pid, uid, client = verified_client (source)
  if not uid or actual_pid ~= owner_pid then return false, nil end
  if app_pid == owner_pid then return true, uid end
  -- Pulse streams belong to the authenticated broker Client; the engine pins
  -- its /proc executable and birth stamp before submitting this exact lease.
  return client ["client.api"] == "pipewire-pulse" and
      client ["config.name"] == "pipewire-pulse.conf" and
      client ["pulse.server.type"] == "unix" and
      client ["application.process.id"] == app_pid and
      source.properties ["application.process.id"] == app_pid, uid
end

local function node_by_id (id)
  for node in nodes:iterate () do
    if node ["bound-id"] == id then return node end
  end
  return nil
end

local function source_by_serial (serial)
  for node in nodes:iterate () do
    if node.properties ["object.serial"] == serial and
        node.properties ["media.class"] == "Stream/Output/Audio" then
      return node
    end
  end
  return nil
end

local function verified_lease_nodes (subject, source_serial, value)
  local endpoint_serial, listed_source, app_pid, app_start,
      source_owner_pid, owner_start, engine_pid, engine_start, nonce =
      value:match ("^(%d+):(%d+):(%d+):(%d+):(%d+):(%d+):(%d+):(%d+):(%x+)$")
  if not endpoint_serial or listed_source ~= source_serial or
      not app_start or not owner_start or not engine_start or #nonce ~= 32 then
    return nil
  end
  local endpoint = node_by_id (subject)
  local source = source_by_serial (source_serial)
  if not endpoint or not source then return nil end
  local ep = endpoint.properties
  local actual_engine_pid, engine_uid = verified_client (endpoint)
  local source_matches, source_uid = source_owner_matches (source, app_pid, source_owner_pid)
  if ep ["object.serial"] ~= endpoint_serial or
      ep ["media.class"] ~= "Audio/Sink" or
      ep ["application.id"] ~= "io.mixrelay.driver" or
      not engine_uid or not source_uid or engine_uid ~= source_uid or
      actual_engine_pid ~= engine_pid or not source_matches then
    return nil
  end
  return endpoint, source
end

local function linkable_for_node (om, node)
  local matched = nil
  for si in om:iterate { type = "SiLinkable" } do
    local associated = si:get_associated_proxy ("node")
    if associated and associated ["bound-id"] == node ["bound-id"] and
        associated.properties ["object.serial"] == node.properties ["object.serial"] then
      if matched then return nil end
      matched = si
    end
  end
  return matched
end

local function restore_previous_link (subject, source_serial, previous)
  local endpoint, source = verified_lease_nodes (subject, source_serial, previous)
  if not endpoint then
    log:debug ("released lease no longer identifies live source and endpoint")
    return
  end
  local event_source = Plugin.find ("standard-event-source")
  if not event_source then return end
  local om = event_source:call ("get-object-manager", "session-item")
  if not om then return end
  local source_si = linkable_for_node (om, source)
  local endpoint_si = linkable_for_node (om, endpoint)
  if not source_si or not endpoint_si then
    log:debug ("released lease has no matching policy session items")
    return
  end

  local source_flags = lutils:get_flags (source_si.id)
  -- An external reroute owns a different peer; never remove that link.
  if source_flags.peer_id ~= endpoint_si.id then
    log:debug (source_si, "released lease no longer owns the policy peer")
    return
  end
  log:debug (source_si, "restoring released source-mute policy link")
  local link = lutils.lookupLink (source_si.id, endpoint_si.id)
  if not link then return end
  -- The lease removed native links, while this exact policy link still owns
  -- its source/peer pair. Reactivate it without selecting a new destination.
  -- A policy's follow-default metadata may have changed since initial linking;
  -- recreating the session item would lose the route we temporarily suppressed.
  link:deactivate (Feature.SessionItem.ACTIVE)
  link:activate (Feature.SessionItem.ACTIVE, function (l, error)
    if error then
      log:warning (l, "released source-mute link could not be restored: " .. tostring (error))
    end
  end)
end

local function changed (_, subject, key, _, value)
  local source_serial = key and key:match ("^lease%.(%d+)$")
  if not source_serial then return end
  local ack_key = "ack." .. source_serial
  local by_source = leases [subject] or {}
  local previous = by_source [source_serial]
  by_source [source_serial] = nil
  leases [subject] = by_source
  metadata:set (subject, ack_key, nil, nil)
  if not value then
    if previous then
      restore_previous_link (subject, source_serial, previous)
      rescan_linking ()
    end
    return
  end

  local endpoint = verified_lease_nodes (subject, source_serial, value)
  if not endpoint then return end

  -- This in-memory write precedes the ACK. A select-target hook cannot observe
  -- an acknowledged lease without seeing the same exact serial pair here.
  by_source [source_serial] = value
  metadata:set (subject, ack_key, nil, value)
end

metadata:activate (Features.ALL, function (m, error)
  if error then
    log:warning ("source-mute metadata activation failed: " .. tostring (error))
    Script:finish_activation_with_error ("MixRelay metadata activation failed: " .. tostring (error))
    return
  end
  log:info (m, "source-mute metadata activation callback succeeded")
  m:connect ("changed", changed)
  SimpleEventHook {
    name = "mixrelay/suppress-owned-forwarding",
    after = "linking/prepare-link",
    before = "linking/link-target",
    interests = {
      EventInterest { Constraint { "event.type", "=", "select-target" } },
    },
    execute = function (event)
      local _, _, si, si_props, _, target = lutils:unwrap_select_target_event (event)
      if not target or si_props ["item.node.direction"] ~= "output" then return end
      local source = si:get_associated_proxy ("node")
      local endpoint = target:get_associated_proxy ("node")
      if not source or not endpoint then return end
      local by_source = leases [endpoint ["bound-id"]]
      if not by_source then return end
      local source_serial = source.properties ["object.serial"]
      local value = by_source [source_serial]
      if not value then return end
      local endpoint_serial, listed_source, app_pid, app_start,
          source_owner_pid, owner_start, engine_pid =
          value:match ("^(%d+):(%d+):(%d+):(%d+):(%d+):(%d+):(%d+):")
      local actual_engine_pid, engine_uid = verified_client (endpoint)
      local source_matches, source_uid = source_owner_matches (source, app_pid, source_owner_pid)
      if endpoint.properties ["object.serial"] ~= endpoint_serial or
          source_serial ~= listed_source or
          not app_start or not owner_start or
          endpoint.properties ["application.id"] ~= "io.mixrelay.driver" or
          not engine_uid or not source_uid or engine_uid ~= source_uid or
          actual_engine_pid ~= engine_pid or not source_matches then
        return
      end
      event:set_data ("target", nil)
      log:info (si, "suppressing managed link to the leased MixRelay endpoint")
    end,
  }:register ()
  metadata:set (0, "hook.version", nil, "1")
  log:info (m, "source-mute hook registered; finishing activation")
  Script:finish_activation ()
end)
