terraform {
  required_version = ">= 1.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

variable "aws_access_key_id" {
  type      = string
  sensitive = true
}

variable "aws_secret_access_key" {
  type      = string
  sensitive = true
}

provider "aws" {
  region     = "us-east-2"
  access_key = var.aws_access_key_id
  secret_key = var.aws_secret_access_key
}

data "aws_caller_identity" "current" {}

resource "aws_iam_policy" "claude_question_s3" {
  name = "claude-question-s3"

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = ["s3:*", "s3-object-lambda:*"]
        Resource = [
          aws_s3_bucket.claude_question.arn,
          "${aws_s3_bucket.claude_question.arn}/*",
        ]
      },
      {
        Effect = "Allow"
        Action = [
          "iam:GetPolicy",
          "iam:GetPolicyVersion",
          "iam:DeletePolicy",
          "iam:CreatePolicyVersion",
          "iam:DeletePolicyVersion",
          "iam:ListPolicyVersions",
        ]
        Resource = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:policy/claude-question-s3"
      }
    ]
  })
}

resource "aws_s3_bucket" "claude_question" {
  bucket = "claude-question-infra"
}

resource "aws_s3_bucket_public_access_block" "claude_question" {
  bucket = aws_s3_bucket.claude_question.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}
