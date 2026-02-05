# Terraform configuration for low-latency trading infrastructure
# Deploy to us-east-1 for minimum latency to Polymarket

terraform {
  required_version = ">= 1.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.aws_region
}

# Variables
variable "aws_region" {
  description = "AWS region for deployment"
  default     = "us-east-1"
}

variable "instance_type" {
  description = "EC2 instance type"
  default     = "c6i.large"
}

variable "key_name" {
  description = "SSH key pair name"
  default     = "trading-bot"
}

variable "allowed_ssh_cidr" {
  description = "CIDR block allowed to SSH (your IP)"
  default     = "0.0.0.0/0"  # CHANGE THIS to your IP/32
}

# Get latest Amazon Linux 2023 AMI
data "aws_ami" "amazon_linux_2023" {
  most_recent = true
  owners      = ["amazon"]

  filter {
    name   = "name"
    values = ["al2023-ami-*-x86_64"]
  }

  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

# VPC (use default for simplicity, or create dedicated)
data "aws_vpc" "default" {
  default = true
}

data "aws_subnets" "default" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.default.id]
  }
}

# Security Group
resource "aws_security_group" "trading_bot" {
  name        = "trading-bot-sg"
  description = "Security group for trading bot"
  vpc_id      = data.aws_vpc.default.id

  # SSH access (restrict to your IP in production)
  ingress {
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = [var.allowed_ssh_cidr]
    description = "SSH access"
  }

  # All outbound traffic (for API calls)
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
    description = "All outbound traffic"
  }

  tags = {
    Name = "trading-bot-sg"
  }
}

# IAM Role for EC2 (CloudWatch, Secrets Manager access)
resource "aws_iam_role" "trading_bot" {
  name = "trading-bot-role"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Action = "sts:AssumeRole"
        Effect = "Allow"
        Principal = {
          Service = "ec2.amazonaws.com"
        }
      }
    ]
  })
}

resource "aws_iam_role_policy" "trading_bot" {
  name = "trading-bot-policy"
  role = aws_iam_role.trading_bot.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "logs:CreateLogGroup",
          "logs:CreateLogStream",
          "logs:PutLogEvents"
        ]
        Resource = "arn:aws:logs:*:*:*"
      },
      {
        Effect = "Allow"
        Action = [
          "secretsmanager:GetSecretValue"
        ]
        Resource = "arn:aws:secretsmanager:${var.aws_region}:*:secret:trading-bot/*"
      },
      {
        Effect = "Allow"
        Action = [
          "cloudwatch:PutMetricData"
        ]
        Resource = "*"
      }
    ]
  })
}

resource "aws_iam_instance_profile" "trading_bot" {
  name = "trading-bot-profile"
  role = aws_iam_role.trading_bot.name
}

# EC2 Instance
resource "aws_instance" "trading_bot" {
  ami                         = data.aws_ami.amazon_linux_2023.id
  instance_type               = var.instance_type
  key_name                    = var.key_name
  vpc_security_group_ids      = [aws_security_group.trading_bot.id]
  subnet_id                   = data.aws_subnets.default.ids[0]
  iam_instance_profile        = aws_iam_instance_profile.trading_bot.name
  associate_public_ip_address = true

  # Enable enhanced networking
  ebs_optimized = true

  root_block_device {
    volume_type           = "gp3"
    volume_size           = 20
    iops                  = 3000
    throughput            = 125
    delete_on_termination = true
    encrypted             = true
  }

  # Placement group for network performance (optional)
  # placement_group = aws_placement_group.trading.id

  tags = {
    Name        = "trading-bot"
    Environment = "production"
    Purpose     = "polymarket-arbitrage"
  }

  lifecycle {
    ignore_changes = [ami]  # Don't recreate on AMI updates
  }
}

# Elastic IP for stable address
resource "aws_eip" "trading_bot" {
  instance = aws_instance.trading_bot.id
  domain   = "vpc"

  tags = {
    Name = "trading-bot-eip"
  }
}

# CloudWatch Log Group
resource "aws_cloudwatch_log_group" "trading_bot" {
  name              = "/trading-bot/application"
  retention_in_days = 14

  tags = {
    Name = "trading-bot-logs"
  }
}

# CloudWatch Alarm for high error rate
resource "aws_cloudwatch_metric_alarm" "error_rate" {
  alarm_name          = "trading-bot-error-rate"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 2
  metric_name         = "ErrorCount"
  namespace           = "TradingBot"
  period              = 300
  statistic           = "Sum"
  threshold           = 10
  alarm_description   = "High error rate in trading bot"

  dimensions = {
    InstanceId = aws_instance.trading_bot.id
  }
}

# Outputs
output "instance_id" {
  value = aws_instance.trading_bot.id
}

output "public_ip" {
  value = aws_eip.trading_bot.public_ip
}

output "ssh_command" {
  value = "ssh -i ~/.ssh/${var.key_name}.pem ec2-user@${aws_eip.trading_bot.public_ip}"
}

output "region" {
  value = var.aws_region
}
