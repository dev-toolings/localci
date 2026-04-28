<?php

declare(strict_types=1);

namespace App;

final class Kernel
{
    public function __construct(private readonly string $environment)
    {
    }

    public function environment(): string
    {
        return $this->environment;
    }
}
