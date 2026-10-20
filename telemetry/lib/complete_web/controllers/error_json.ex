defmodule CompleteWeb.ErrorJSON do
  @moduledoc """
  Phoenix error renderer. Translates plug/parser errors into the frozen
  protocol's error shape `%{error: %{code, message}}`.

    * `413.json` — body over 256 KB -> code `too_large`
    * `400.json` — malformed JSON body -> code `invalid_request`
    * anything else — Phoenix status text under generic code `error`

  To change upstream error mapping, edit these `render/2` clauses.
  """

  @doc """
  Renders an HTTP error template as a protocol-shaped JSON body.

    * `"413.json"` — body over 256 KB -> code `too_large`
    * `"400.json"` — malformed JSON body -> code `invalid_request`
    * anything else — Phoenix status text under generic code `error`
  """
  def render("413.json", _assigns) do
    %{error: %{code: "too_large", message: "request body exceeds 256 KB limit"}}
  end

  def render("400.json", _assigns) do
    %{error: %{code: "invalid_request", message: "malformed request"}}
  end

  def render(template, _assigns) do
    %{error: %{code: "error", message: Phoenix.Controller.status_message_from_template(template)}}
  end
end
