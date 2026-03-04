from django.db import models


class Session(models.Model):
    session_id = models.TextField(primary_key=True)
    started_at = models.TextField(null=True)
    ended_at = models.TextField(null=True)
    start_reason = models.TextField(null=True)
    end_reason = models.TextField(null=True)
    cwd = models.TextField(null=True)
    transcript_path = models.TextField(null=True)

    class Meta:
        managed = False
        db_table = "sessions"


class ToolUse(models.Model):
    id = models.AutoField(primary_key=True)
    tool_use_id = models.TextField(null=True)
    session_id = models.TextField(null=True)
    tool_name = models.TextField(null=True)
    timestamp = models.TextField(null=True)
    cwd = models.TextField(null=True)
    input = models.TextField(null=True)
    response_summary = models.TextField(null=True)

    class Meta:
        managed = False
        db_table = "tool_uses"


class Prompt(models.Model):
    id = models.AutoField(primary_key=True)
    session_id = models.TextField(null=True)
    timestamp = models.TextField(null=True)
    prompt_text = models.TextField(null=True)

    class Meta:
        managed = False
        db_table = "prompts"


class TokenUsage(models.Model):
    id = models.AutoField(primary_key=True)
    session_id = models.TextField(null=True)
    timestamp = models.TextField(null=True)
    model = models.TextField(null=True)
    input_tokens = models.IntegerField(default=0)
    cache_creation_tokens = models.IntegerField(default=0)
    cache_read_tokens = models.IntegerField(default=0)
    output_tokens = models.IntegerField(default=0)
    api_call_count = models.IntegerField(default=0)
    last_transcript_offset = models.IntegerField(default=0)

    class Meta:
        managed = False
        db_table = "token_usage"


class Plan(models.Model):
    id = models.AutoField(primary_key=True)
    session_id = models.TextField(null=True)
    tool_use_id = models.TextField(null=True)
    timestamp = models.TextField(null=True)
    plan_text = models.TextField(null=True)
    accepted = models.IntegerField(null=True)

    class Meta:
        managed = False
        db_table = "plans"
