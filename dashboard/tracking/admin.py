from django.contrib import admin

from .models import Plan, Prompt, Session, TokenUsage, ToolUse


@admin.register(Session)
class SessionAdmin(admin.ModelAdmin):
    list_display = ("session_id", "started_at", "ended_at", "start_reason", "cwd")
    list_filter = ("start_reason", "end_reason")
    search_fields = ("session_id", "cwd")


@admin.register(ToolUse)
class ToolUseAdmin(admin.ModelAdmin):
    list_display = ("id", "tool_name", "session_id", "timestamp", "cwd")
    list_filter = ("tool_name",)
    search_fields = ("tool_name", "session_id")


@admin.register(Prompt)
class PromptAdmin(admin.ModelAdmin):
    list_display = ("id", "session_id", "timestamp", "prompt_text")
    search_fields = ("prompt_text", "session_id")


@admin.register(TokenUsage)
class TokenUsageAdmin(admin.ModelAdmin):
    list_display = (
        "id",
        "session_id",
        "model",
        "input_tokens",
        "output_tokens",
        "api_call_count",
    )
    list_filter = ("model",)
    search_fields = ("session_id",)


@admin.register(Plan)
class PlanAdmin(admin.ModelAdmin):
    list_display = ("id", "session_id", "tool_use_id", "timestamp")
    search_fields = ("session_id", "plan_text")
