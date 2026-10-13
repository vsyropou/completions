defmodule CompleteWeb.ErrorJSON do
  @moduledoc false

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
