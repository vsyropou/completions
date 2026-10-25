defmodule CompleteWeb.EventIndexLive do
  @moduledoc "Events browser: stats cards up top, filter/paginate/reconcile below."
  use Phoenix.LiveView, layout: false

  alias Complete.Telemetry

  @page 50

  @impl true
  def mount(_params, _session, socket) do
    {:ok,
     socket
     |> assign(
       events: Telemetry.recent_events(@page),
       q: "",
       language: "",
       outcome: "",
       request_id: nil,
       languages: Telemetry.distinct_languages(),
       offset: 0,
       page_size: @page
     )
     |> refresh_stats()}
  end

  @impl true
  def handle_event("filter", %{"q" => q, "language" => lang, "outcome" => outcome}, socket) do
    {:noreply,
     socket
     |> assign(q: q, language: lang, outcome: outcome, request_id: nil, offset: 0)
     |> reload()}
  end

  def handle_event("request", %{"id" => id}, socket) do
    {:noreply,
     socket
     |> assign(q: "", language: "", outcome: "", request_id: id, offset: 0)
     |> assign(events: Telemetry.by_request_id(id))}
  end

  def handle_event("clear", _params, socket) do
    {:noreply,
     socket
     |> assign(q: "", language: "", outcome: "", request_id: nil, offset: 0)
     |> reload()}
  end

  def handle_event("next", _params, socket) do
    offset = socket.assigns.offset + @page
    {:noreply, socket |> assign(offset: offset) |> reload()}
  end

  def handle_event("prev", _params, socket) do
    offset = max(socket.assigns.offset - @page, 0)
    {:noreply, socket |> assign(offset: offset) |> reload()}
  end

  def handle_event("refresh", _params, socket) do
    {:noreply, socket |> refresh_stats() |> reload()}
  end

  defp refresh_stats(socket) do
    assign(socket,
      requests_non_cancelled: Telemetry.count_non_cancelled(),
      acceptance_rate: Telemetry.acceptance_rate(),
      by_language: Telemetry.by_language()
    )
  end

  defp reload(socket) do
    if socket.assigns.request_id do
      assign(socket, events: Telemetry.by_request_id(socket.assigns.request_id))
    else
      opts =
        [
          q: nullify(socket.assigns.q),
          language: nullify(socket.assigns.language),
          outcome: nullify(socket.assigns.outcome),
          limit: @page,
          offset: socket.assigns.offset
        ]
        |> Enum.reject(&(elem(&1, 1) == nil))

      assign(socket, events: Telemetry.search_events(opts))
    end
  end

  defp nullify(""), do: nil
  defp nullify(s) when is_binary(s), do: s

  defp tag_class("requested"), do: "tag tag-requested"
  defp tag_class("served"), do: "tag tag-served"
  defp tag_class("accepted"), do: "tag tag-accepted"
  defp tag_class("cancelled"), do: "tag tag-cancelled"
  defp tag_class("superseded"), do: "tag tag-superseded"
  defp tag_class("error"), do: "tag tag-error"
  defp tag_class(_), do: "tag"

  @impl true
  def render(assigns) do
    ~H"""
    <h1>Telemetry</h1>

    <div class="cards">
      <div class="card">
        <div class="label">Non-cancelled requests</div>
        <div class="big"><%= @requests_non_cancelled %></div>
      </div>
      <div class="card">
        <div class="label">Acceptance rate</div>
        <div class="big"><%= @acceptance_rate %>%</div>
        <div class="bar">
          <div class="fill" style={"width: #{min(@acceptance_rate, 100.0)}%;"}></div>
        </div>
      </div>
      <div class="card">
        <div class="label">By language</div>
        <div :for={{lang, n} <- Enum.sort_by(@by_language, &elem(&1, 1), :desc)}>
          <%= lang %>: <%= n %>
        </div>
      </div>
      <button phx-click="refresh">↻ refresh stats</button>
    </div>

    <form phx-submit="filter" class="filter">
      <input type="text" name="q" value={@q} placeholder="search prompt/response…" />
      <select name="language">
        <option value="">all languages</option>
        <option :for={lang <- @languages} value={lang} selected={@language == lang}>
          <%= lang %>
        </option>
      </select>
      <select name="outcome">
        <option value="">all outcomes</option>
        <option
          :for={o <- ~w(requested served superseded accepted cancelled error)}
          value={o}
          selected={@outcome == o}
        >
          <%= o %>
        </option>
      </select>
      <button type="submit">Filter</button>
      <button type="button" phx-click="clear">Clear</button>
    </form>

    <div :if={@request_id} style="margin-bottom: 0.75rem;">
      Reconciling request <code><%= @request_id %></code>
      — <a phx-click="clear" class="rid-link">← back to list</a>
    </div>

    <table>
      <thead>
        <tr>
          <th>inserted_at</th>
          <th>request_id</th>
          <th>outcome</th>
          <th>latency_ms</th>
          <th>language</th>
          <th>file_path</th>
        </tr>
      </thead>
      <tbody>
        <tr :for={e <- @events} id={"ev-#{e.id}"}>
          <td><%= Calendar.strftime(e.inserted_at, "%H:%M:%S.%f") %></td>
          <td class="rid">
            <a phx-click="request" phx-value-id={e.request_id}><%= e.request_id %></a>
          </td>
          <td><span class={tag_class(e.outcome)}><%= e.outcome %></span></td>
          <td><%= if e.latency_ms && e.latency_ms > 0, do: e.latency_ms, else: "" %></td>
          <td><%= e.language %></td>
          <td>
            <%= e.file_path %>
            <details>
              <summary>prompt / response</summary>
              <pre><%= e.prompt %></pre>
              <pre><%= e.response %></pre>
            </details>
          </td>
        </tr>
      </tbody>
    </table>

    <div :if={!@request_id} style="margin-top: 1rem;">
      <button phx-click="prev" disabled={@offset == 0}>← prev</button>
      <span style="margin: 0 0.5rem;">offset <%= @offset %></span>
      <button phx-click="next" disabled={length(@events) < @page_size}>next →</button>
    </div>
    """
  end
end
